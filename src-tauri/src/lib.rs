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
}

impl AppState {
    pub fn new(dir: std::path::PathBuf, data: AppData) -> Self {
        Self {
            dir: dir.clone(),
            data: Mutex::new(data),
            git: git_sync::GitManager::new(dir),
        }
    }
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

            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
                if let Some(state) = window.try_state::<AppState>() {
                    if let (Ok(pos), Ok(size)) = (window.outer_position(), window.outer_size()) {
                        let scale = window.scale_factor().unwrap_or(1.0);
                        let logical_h = size.height as f64 / scale;
                        let logical_w = size.width as f64 / scale;

                        let (snapshot, changed) = {
                            let mut d = state.data.lock().unwrap();
                            let mut changed = false;
                            if d.settings.window.position != Some((pos.x, pos.y)) {
                                d.settings.window.position = Some((pos.x, pos.y));
                                changed = true;
                            }
                            // 收起状态下的高度不写入记忆值
                            if logical_h >= model::MIN_EXPANDED_HEIGHT
                                && d.settings.window.size != Some((logical_w, logical_h))
                            {
                                d.settings.window.size = Some((logical_w, logical_h));
                                changed = true;
                            }
                            (d.clone(), changed)
                        };

                        // 仅在位置/尺寸真正变化时落盘，避免拖动过程中反复写盘
                        if changed {
                            let _ = store::save(&state.dir, &snapshot);
                        }
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
            commands::move_window,
            commands::save_window_state,
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

    app.run(|_handle, event| {
        if let RunEvent::ExitRequested { api, code, .. } = event {
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
