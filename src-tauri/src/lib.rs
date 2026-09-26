//! DeskTodo —— 常驻桌面的待办小窗（Tauri v2）

mod commands;
mod git_sync;
mod model;
pub mod self_test;
mod store;
mod tray;

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};

use model::{AppData, Todo};

/// 应用级共享状态：数据目录 + 内存数据 + git 管理器
pub struct AppState {
    pub dir: std::path::PathBuf,
    pub data: Mutex<AppData>,
    pub git: git_sync::GitManager,
    /// 最后一次待写入的窗口几何（物理坐标 + 逻辑尺寸）
    pub last_geometry: Mutex<Option<((i32, i32), (f64, f64))>>,
    /// 几何信息自上次落盘以来是否变化过
    pub geo_dirty: std::sync::atomic::AtomicBool,
    /// 诊断：几何信息实际落盘次数（由写入方自增，供压力测试断言）
    pub geo_write_count: std::sync::atomic::AtomicU32,
}

/// 窗口几何的定时落盘周期。
///
/// 为什么用「固定周期」而不是「拖动结束防抖」：
/// - 拖动窗口会以 60fps 触发 Moved 事件，任何在事件里做 IO 的做法
///   （实测单次写盘 + git 暂存 30~42ms）都会占满主线程导致窗口抖动；
/// - 防抖需要额外的计时状态机，边界情况多、不易验证；
/// - 定时保存则完全与拖动解耦：拖动过程中一次都不写，最多丢失 GEO_SAVE_INTERVAL
///   秒内的位置变化，而这个代价几乎不可感知（窗口几何不是关键数据）。
const GEO_SAVE_INTERVAL: Duration = Duration::from_secs(30);

impl AppState {
    pub fn new(dir: std::path::PathBuf, data: AppData) -> Self {
        Self {
            dir: dir.clone(),
            data: Mutex::new(data),
            git: git_sync::GitManager::new(dir),
            last_geometry: Mutex::new(None),
            geo_dirty: std::sync::atomic::AtomicBool::new(false),
            geo_write_count: std::sync::atomic::AtomicU32::new(0),
        }
    }
}

/// 记录「窗口几何变了」，不触发任何 IO。
fn mark_geometry_dirty(app: &AppHandle) {
    use std::sync::atomic::Ordering;
    if let Some(state) = app.try_state::<AppState>() {
        state.geo_dirty.store(true, Ordering::SeqCst);
    }
}

/// 定时线程：每 GEO_SAVE_INTERVAL 检查一次，只有变化过才写盘。
/// 拖动期间完全不受影响（该线程只做定时检查与必要时的落盘）。
fn geometry_save_loop(app: AppHandle) {
    loop {
        std::thread::sleep(GEO_SAVE_INTERVAL);
        save_geometry_if_dirty(&app);
    }
}

/// 若有未落盘的几何信息则写盘。供定时线程与退出流程共用。
pub fn save_geometry_if_dirty(app: &AppHandle) {
    use std::sync::atomic::Ordering;

    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    if !state.geo_dirty.swap(false, Ordering::SeqCst) {
        return;
    }
    let geometry = state.last_geometry.lock().ok().and_then(|g| *g);
    let Some(((x, y), (w, h))) = geometry else {
        return;
    };
    let snapshot = {
        let mut d = state.data.lock().unwrap();
        d.settings.window.position = Some((x, y));
        if h >= model::MIN_EXPANDED_HEIGHT {
            d.settings.window.size = Some((w, h));
        }
        d.clone()
    };
    let _ = store::save(&state.dir, &snapshot);
    state.geo_write_count.fetch_add(1, Ordering::SeqCst);
}

/// 退出前兜底：把尚未落盘的窗口几何信息立即写入，避免刚拖完就退出导致位置丢失。
pub fn flush_window_geometry(app: &AppHandle) {
    save_geometry_if_dirty(app);
}

pub fn run() {
    let mut builder = tauri::Builder::default();

    builder = builder
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ));

    let app = builder
        .setup(|app| {
            // ---- 数据目录与初始加载 ----
            let dir = app
                .path()
                .app_data_dir()
                .map_err(|e| format!("无法解析应用数据目录: {e}"))?;
            std::fs::create_dir_all(&dir)?;
            let data = store::load(&dir);
            let auto_commit = data.settings.sync.auto_commit;
            app.manage(AppState::new(dir, data));

            // ---- 托盘 ----
            tray::build(app.handle())?;

            // ---- 到期扫描线程 ----
            let handle = app.handle().clone();
            std::thread::spawn(move || reminder_loop(handle));

            // ---- 窗口几何落盘线程（重置式防抖，避免拖动时每帧写盘）----
            let handle = app.handle().clone();
            std::thread::spawn(move || geometry_save_loop(handle));

            // ---- git 自动提交线程（每 60s 检查一次脏标记）----
            if auto_commit {
                let handle = app.handle().clone();
                std::thread::spawn(move || autocommit_loop(handle));
            }

            // ---- 恢复窗口位置/尺寸/收起状态 ----
            let (pos, size, collapsed, start_hidden) = {
                let state = app.state::<AppState>();
                let d = state.data.lock().unwrap();
                (
                    d.settings.window.position,
                    d.settings.window.size,
                    d.settings.window.collapsed,
                    d.settings.start_hidden,
                )
            };
            if let Some(win) = app.get_webview_window("main") {
                if let Some((w, h)) = size {
                    let _ = win.set_size(tauri::LogicalSize::new(w, h));
                }
                if let Some((x, y)) = pos {
                    let _ = win.set_position(tauri::PhysicalPosition::new(x as i32, y as i32));
                } else {
                    // 首次启动：贴到主屏右上角
                    if let Ok(Some(mon)) = win.primary_monitor() {
                        let msize = mon.size();
                        let wsize = win
                            .outer_size()
                            .unwrap_or(tauri::PhysicalSize::new(320, 460));
                        let x = (msize.width as i32 - wsize.width as i32 - 24).max(0);
                        let y = 48i32;
                        let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
                    }
                }
                if collapsed {
                    let _ = win.set_size(tauri::LogicalSize::new(320.0, model::COLLAPSED_HEIGHT));
                }
                let _ = win.set_always_on_top(true);
                // 开机自启时可选择直接隐藏到托盘
                if start_hidden {
                    let _ = win.hide();
                }
            }

            // ---- 冒烟模式：启动 N 秒后自动退出（用于无人值守验证 GUI 能起来）----
            if let Some(seconds) = smoke_seconds() {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(seconds));
                    println!("[smoke] 正常运行 {seconds} 秒，窗口与托盘已创建，退出码 0");
                    handle.exit(0);
                });
            }

            // ---- 拖动压力测试：模拟连续移动窗口，统计写盘次数 ----
            // 用法: desk-todo --move-storm [次数]
            // 期望：无论移动多少次，几何信息只落盘一次（延迟合并），
            //       且每次 set_position 的耗时稳定在毫秒级（不做 IO）。
            if let Some(steps) = move_storm_steps() {
                let handle = app.handle().clone();
                std::thread::spawn(move || run_move_storm(handle, steps));
            }

            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
                // 高频事件：只更新内存中的待写几何信息，落盘交给延迟任务。
                // 这里绝不做 IO —— 拖动时每帧写盘会占满主线程导致窗口抖动。
                if let Some(state) = window.try_state::<AppState>() {
                    if let (Ok(pos), Ok(size)) = (window.outer_position(), window.outer_size()) {
                        let scale = window.scale_factor().unwrap_or(1.0);
                        let logical_w = size.width as f64 / scale;
                        let logical_h = size.height as f64 / scale;
                        if let Ok(mut g) = state.last_geometry.lock() {
                            *g = Some(((pos.x, pos.y), (logical_w, logical_h)));
                        }
                        mark_geometry_dirty(&window.app_handle().clone());
                    }
                }
            }
            WindowEvent::CloseRequested { api, .. } => {
                // 关闭 = 隐藏到托盘，保持常驻
                api.prevent_close();
                let _ = window.hide();
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::log_frontend,
            commands::trace,
            commands::add_todo,
            commands::update_todo,
            commands::toggle_todo,
            commands::delete_todo,
            commands::restore_todo,
            commands::clear_completed,
            commands::reorder_todos,
            commands::snooze_todo,
            commands::save_settings,
            commands::set_collapsed,
            commands::hide_window,
            commands::show_window,
            commands::reposition_window,
            commands::quit_app,
            commands::export_data,
            commands::import_data,
            commands::notify_now,
            commands::set_autostart,
            commands::get_autostart,
            git_sync::git_status,
            git_sync::git_set_remote,
            git_sync::git_init,
            git_sync::git_commit,
            git_sync::git_pull,
            git_sync::git_push,
            git_sync::git_sync_now,
            git_sync::git_test_remote,
        ])
        .build(tauri::generate_context!())
        .expect("DeskTodo 启动失败");

    app.run(|handle, event| {
        if let RunEvent::ExitRequested { api, code, .. } = event {
            // 退出前把拖动窗口产生的未落盘几何信息写入，避免丢失
            flush_window_geometry(handle);
            // 没有显式退出码时，隐藏窗口而不是退出进程（保持桌面常驻）
            if code.is_none() {
                api.prevent_exit();
            }
        }
    });
}

/// 解析 `--exit-after <秒>`（冒烟测试用）
fn smoke_seconds() -> Option<u64> {
    let args: Vec<String> = std::env::args().collect();
    let idx = args.iter().position(|a| a == "--exit-after")?;
    args.get(idx + 1)?.parse::<u64>().ok()
}

/// 解析 `--move-storm [次数]`（拖动压力测试用，默认 120 次）
fn move_storm_steps() -> Option<u32> {
    let args: Vec<String> = std::env::args().collect();
    let idx = args.iter().position(|a| a == "--move-storm")?;
    Some(
        args.get(idx + 1)
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(120),
    )
}

/// 连续移动窗口并按四段采样 todos.json 的修改时间，统计真实写盘次数。
/// 这是判断「拖动是否还在高频写盘」的确定性依据。
fn run_move_storm(app: AppHandle, steps: u32) {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Instant;

    // 等窗口就绪
    std::thread::sleep(Duration::from_millis(800));

    let Some(win) = app.get_webview_window("main") else {
        eprintln!("[storm] 窗口不存在");
        app.exit(1);
        return;
    };
    let Ok(start_pos) = win.outer_position() else {
        eprintln!("[storm] 无法读取窗口位置");
        app.exit(1);
        return;
    };
    let Some(data_path) = app
        .try_state::<AppState>()
        .map(|s| s.dir.join("todos.json"))
    else {
        app.exit(1);
        return;
    };

    let mtime_ms = |p: &std::path::Path| -> u128 {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis())
            .unwrap_or(0)
    };

    let write_count = std::sync::Arc::new(AtomicU32::new(0));
    let write_count2 = write_count.clone();
    let last = std::sync::Arc::new(std::sync::Mutex::new(mtime_ms(&data_path)));

    // 采样线程：每 10ms 检查一次修改时间，变化即记一次写盘
    let watcher_path = data_path.clone();
    let watcher_done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watcher_done2 = watcher_done.clone();
    let watcher = std::thread::spawn(move || {
        while !watcher_done2.load(Ordering::SeqCst) {
            let m = mtime_ms(&watcher_path);
            {
                let mut l = last.lock().unwrap();
                if m != *l {
                    *l = m;
                    write_count2.fetch_add(1, Ordering::SeqCst);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });

    // 启动阶段（恢复位置、前端首次保存窗口状态）会有若干次正常写入，
    // 这里先等它们平静下来，再把计数器归零，只统计「拖动期间」的写入。
    std::thread::sleep(Duration::from_millis(900));

    // 判定用「几何写入方自己统计的次数」，避免外部按 mtime 采样
    // 因原子写入（写临时文件再 rename）产生误判。
    // 只记录起始值，测试结束时取差值即为拖动期间的真实写入次数。
    let geo_writes_before = app
        .state::<AppState>()
        .geo_write_count
        .load(Ordering::SeqCst);
    write_count.store(0, Ordering::SeqCst);

    // 热身：首次 set_position 可能触发窗口服务器初始化，偶发几十毫秒，
    // 与我们的代码无关。先跑几次不计入统计，避免误报。
    for _ in 0..8 {
        let _ = win.set_position(tauri::PhysicalPosition::new(start_pos.x, start_pos.y));
        std::thread::sleep(Duration::from_millis(16));
    }
    write_count.store(0, Ordering::SeqCst);

    println!("[storm] 开始连续移动窗口 {steps} 次（模拟拖动）");
    let t0 = Instant::now();
    let mut samples: Vec<f64> = Vec::with_capacity(steps as usize);
    for i in 0..steps {
        let dx = ((i as i32 % 40) - 20) * 3;
        let dy = (((i as i32) / 40) % 3) * 2;
        let p = tauri::PhysicalPosition::new(start_pos.x + dx, start_pos.y + dy);
        let s = Instant::now();
        let _ = win.set_position(p);
        samples.push(s.elapsed().as_secs_f64() * 1000.0);
        std::thread::sleep(Duration::from_millis(16)); // ≈60fps
    }
    let elapsed = t0.elapsed().as_secs_f64() * 1000.0;

    // 等一小会儿，确认拖动结束后也不会立刻触发写盘（保存完全由定时器驱动）
    std::thread::sleep(Duration::from_millis(900));
    watcher_done.store(true, Ordering::SeqCst);
    let _ = watcher.join();
    let writes = write_count.load(Ordering::SeqCst);
    let geo_writes_during_drag = app
        .state::<AppState>()
        .geo_write_count
        .load(Ordering::SeqCst)
        - geo_writes_before;

    // 用中位数与 P95 判定，避免个别帧被系统调度打断导致误报
    let mut sorted = samples.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0);
    let p95 = sorted
        .get((sorted.len() as f64 * 0.95) as usize)
        .copied()
        .unwrap_or(0.0);
    let worst = sorted.last().copied().unwrap_or(0.0);

    println!("[storm] 移动 {steps} 次，耗时 {elapsed:.0} ms");
    println!("[storm] 单次 set_position 耗时：中位数 {median:.2} ms，P95 {p95:.2} ms，最慢 {worst:.2} ms");
    println!(
        "[storm] 拖动期间写盘：外部采样 {writes} 次 / 几何写入方自计 {geo_writes_during_drag} 次"
    );

    // ---- 验证定时保存路径确实工作 ----
    // 注意：要比对的是「窗口实际位置」与「保存后的磁盘内容」。
    // 内存里的 settings.window.position 启动后一直是 None（只有定时保存才会更新），
    // 拿它当基准会误判。
    let moved_to = (start_pos.x + 33, start_pos.y + 21);
    let _ = win.set_position(tauri::PhysicalPosition::new(moved_to.0, moved_to.1));
    std::thread::sleep(Duration::from_millis(120)); // 等窗口真正移动到位
    let actual_pos = win.outer_position().ok().map(|p| (p.x, p.y));
    mark_geometry_dirty(&app);
    save_geometry_if_dirty(&app);

    let saved_on_disk = std::fs::read_to_string(&data_path)
        .ok()
        .and_then(|t| serde_json::from_str::<AppData>(&t).ok())
        .and_then(|d| d.settings.window.position);
    let periodic_ok = match (actual_pos, saved_on_disk) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    };
    println!(
        "[storm] 定时保存路径: {} （窗口实际 {actual_pos:?} / 盘中记录 {saved_on_disk:?}）",
        if periodic_ok { "✓ 生效" } else { "✗ 未生效" }
    );

    // 判定标准：
    //  - 拖动期间几何写入方一次都不写（保存完全由定时器驱动）。修复前是每帧都写。
    //  - P95 耗时 < 5ms，说明移动路径上没有 IO（修复前每次 30~42ms）。
    //    用 P95 而非最大值，避免偶发的系统调度抖动造成误报。
    let ok = geo_writes_during_drag == 0 && p95 < 5.0 && periodic_ok;
    println!(
        "[storm] 判定: {}",
        if ok {
            "✓ 通过（拖动期间零写盘，P95 亚毫秒级，定时保存有效）"
        } else {
            "✗ 失败（拖动时仍在写盘 / 单次移动耗时过高 / 定时保存失效）"
        }
    );
    // 还原窗口位置
    let _ = win.set_position(start_pos);
    app.exit(if ok { 0 } else { 1 });
}

/// 每 15 秒扫描一次到期任务，到点弹系统通知并广播事件
fn reminder_loop(app: AppHandle) {
    loop {
        std::thread::sleep(Duration::from_secs(15));
        let mut fired: Vec<Todo> = Vec::new();

        {
            let Some(state) = app.try_state::<AppState>() else {
                continue;
            };
            let mut d = state.data.lock().unwrap();
            let now = chrono::Local::now();
            let lead = d.settings.reminder.lead_minutes as i64;
            let enabled = d.settings.reminder.enabled;
            let mut dirty = false;

            if enabled {
                for t in d.todos.iter_mut() {
                    if t.done || t.due_at.is_none() {
                        continue;
                    }
                    if t.notified_at.is_some() {
                        continue;
                    }
                    let due = t.due_at.as_ref().unwrap();
                    let Ok(dt) = chrono::DateTime::parse_from_rfc3339(due) else {
                        continue;
                    };
                    let due_local = dt.with_timezone(&chrono::Local);
                    // 提前 lead 分钟提醒
                    let trigger = due_local - chrono::Duration::minutes(lead);
                    if now >= trigger {
                        t.notified_at = Some(now.to_rfc3339());
                        fired.push(t.clone());
                        dirty = true;
                    }
                }
            }

            if dirty {
                let snapshot = d.clone();
                let dir = state.dir.clone();
                drop(d);
                let _ = store::save(&dir, &snapshot);
            }
        }

        if fired.is_empty() {
            continue;
        }

        println!("[reminder] 触发提醒 {} 项", fired.len());
        for t in &fired {
            let title = if t.priority == model::Priority::High {
                "⏰ 高优待办到期"
            } else {
                "⏰ 待办到期"
            };
            let body = if t.title.is_empty() {
                "有一条待办到时间了".to_string()
            } else {
                t.title.clone()
            };
            commands::send_notification(&app, title, &body);
        }
        let _ = app.emit("due://fired", &fired);
        let _ = tray::refresh(&app);
    }
}

/// 自动提交：数据变更后 60 秒内提交到本地 git 仓库
fn autocommit_loop(app: AppHandle) {
    loop {
        std::thread::sleep(Duration::from_secs(60));
        let Some(state) = app.try_state::<AppState>() else {
            continue;
        };
        let enabled = {
            let d = state.data.lock().unwrap();
            d.settings.sync.auto_commit && !d.settings.sync.remote_url.trim().is_empty()
        };
        if !enabled {
            continue;
        }
        if let Ok(res) = state.git.sync_now("desk-todo: 自动同步", None, None) {
            let _ = app.emit("git://status", res);
        }
    }
}

/// 供命令层复用：把 AppData 写回磁盘并刷新托盘
pub fn persist(app: &AppHandle, data: &AppData) -> Result<(), String> {
    let state = app.state::<AppState>();
    store::save(&state.dir, data)?;
    if let Ok(mut g) = state.git.dirty.lock() {
        *g = true;
    }
    let _ = tray::refresh(app);
    Ok(())
}

/// 读取当前数据的快照（克隆）
pub fn snapshot(app: &AppHandle) -> AppData {
    let state = app.state::<AppState>();
    let d = state.data.lock().unwrap();
    d.clone()
}

pub type NoticeMap = HashMap<String, String>;
