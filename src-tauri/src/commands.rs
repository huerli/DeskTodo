//! Tauri 命令层：前端可调用的所有能力

use crate::model::{AppData, Priority, Settings, Todo};
use crate::{persist, snapshot, AppState};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

// ---------------------------------------------------------------- 读取

#[tauri::command]
pub fn get_state(app: AppHandle) -> AppData {
    let mut d = snapshot(&app);
    d.data_dir = app
        .state::<AppState>()
        .dir
        .display()
        .to_string();
    d
}

/// 前端启动轨迹：写入 boot-trace.log（无任何依赖，用于定位启动卡点）
/// 仅在启动阶段调用，文件超过 64KB 时自动清空
#[tauri::command]
pub fn trace(app: AppHandle, step: String) {
    let dir = app.state::<AppState>().dir.clone();
    let path = dir.join("boot-trace.log");
    if std::fs::metadata(&path).map(|m| m.len() > 64 * 1024).unwrap_or(false) {
        let _ = std::fs::write(&path, b"");
    }
    let line = format!(
        "[{}] {}\n",
        chrono::Local::now().format("%H:%M:%S%.3f"),
        step
    );
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// 前端日志落盘：便于排查「界面空白」这类问题
/// 级别: info | warn | error
#[tauri::command]
pub fn log_frontend(app: AppHandle, level: String, message: String) {
    let dir = app.state::<AppState>().dir.clone();
    let line = format!(
        "[{}] [{}] {}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        level,
        message.replace('\n', " ⏎ ")
    );
    let path = dir.join("frontend.log");
    // 控制日志体积：超过 256KB 时清空重写
    if std::fs::metadata(&path).map(|m| m.len() > 256 * 1024).unwrap_or(false) {
        let _ = std::fs::write(&path, b"");
    }
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
    }
}

// ---------------------------------------------------------------- 待办 CRUD

#[derive(Debug, Deserialize)]
pub struct NewTodo {
    pub title: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub due_at: Option<String>,
    #[serde(default)]
    pub priority: Priority,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[tauri::command]
pub fn add_todo(app: AppHandle, payload: NewTodo) -> Result<AppData, String> {
    let title = payload.title.trim().to_string();
    if title.is_empty() {
        return Err("待办内容不能为空".into());
    }

    let state = app.state::<AppState>();
    let data = {
        let mut d = state.data.lock().unwrap();
        let max_order = d.todos.iter().map(|t| t.order).max().unwrap_or(0);
        let now = crate::model::now_rfc3339();
        d.todos.push(Todo {
            id: uuid::Uuid::new_v4().to_string(),
            title,
            notes: payload.notes,
            done: false,
            due_at: payload.due_at.filter(|s| !s.trim().is_empty()),
            priority: payload.priority,
            tags: payload.tags,
            created_at: now.clone(),
            updated_at: now,
            completed_at: None,
            notified_at: None,
            order: max_order + 10,
        });
        crate::model::normalize(&mut d.todos);
        d.clone()
    };
    persist(&app, &data)?;
    Ok(data)
}

#[derive(Debug, Deserialize)]
pub struct TodoPatch {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub due_at: Option<Option<String>>,
    #[serde(default)]
    pub priority: Option<Priority>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

#[tauri::command]
pub fn update_todo(app: AppHandle, payload: TodoPatch) -> Result<AppData, String> {
    let state = app.state::<AppState>();
    let data = {
        let mut d = state.data.lock().unwrap();
        let Some(t) = d.todos.iter_mut().find(|t| t.id == payload.id) else {
            return Err("待办不存在".into());
        };
        if let Some(title) = payload.title {
            let title = title.trim().to_string();
            if title.is_empty() {
                return Err("待办内容不能为空".into());
            }
            t.title = title;
        }
        if let Some(notes) = payload.notes {
            t.notes = notes;
        }
        if let Some(due) = payload.due_at {
            let changed = t.due_at != due;
            t.due_at = due.filter(|s| !s.trim().is_empty());
            // 改期后需要重新提醒
            if changed {
                t.notified_at = None;
            }
        }
        if let Some(p) = payload.priority {
            t.priority = p;
        }
        if let Some(tags) = payload.tags {
            t.tags = tags;
        }
        t.updated_at = crate::model::now_rfc3339();
        d.clone()
    };
    persist(&app, &data)?;
    Ok(data)
}

#[tauri::command]
pub fn toggle_todo(app: AppHandle, id: String, done: Option<bool>) -> Result<AppData, String> {
    let state = app.state::<AppState>();
    let data = {
        let mut d = state.data.lock().unwrap();
        let now = crate::model::now_rfc3339();
        let Some(t) = d.todos.iter_mut().find(|t| t.id == id) else {
            return Err("待办不存在".into());
        };
        t.done = done.unwrap_or(!t.done);
        t.completed_at = if t.done { Some(now.clone()) } else { None };
        t.updated_at = now;
        d.clone()
    };
    persist(&app, &data)?;
    Ok(data)
}

#[tauri::command]
pub fn delete_todo(app: AppHandle, id: String) -> Result<AppData, String> {
    let state = app.state::<AppState>();
    let data = {
        let mut d = state.data.lock().unwrap();
        if let Some(pos) = d.todos.iter().position(|t| t.id == id) {
            let removed = d.todos.remove(pos);
            d.trash.push(removed);
            if d.trash.len() > 50 {
                let overflow = d.trash.len() - 50;
                d.trash.drain(0..overflow);
            }
        }
        d.clone()
    };
    persist(&app, &data)?;
    Ok(data)
}

#[tauri::command]
pub fn restore_todo(app: AppHandle) -> Result<AppData, String> {
    let state = app.state::<AppState>();
    let data = {
        let mut d = state.data.lock().unwrap();
        if let Some(mut t) = d.trash.pop() {
            t.updated_at = crate::model::now_rfc3339();
            t.done = false;
            t.completed_at = None;
            let max_order = d.todos.iter().map(|x| x.order).max().unwrap_or(0);
            t.order = max_order + 10;
            d.todos.push(t);
        }
        d.clone()
    };
    persist(&app, &data)?;
    Ok(data)
}

#[tauri::command]
pub fn clear_completed(app: AppHandle) -> Result<AppData, String> {
    let state = app.state::<AppState>();
    let data = {
        let mut d = state.data.lock().unwrap();
        let (done, keep): (Vec<Todo>, Vec<Todo>) = d.todos.drain(..).partition(|t| t.done);
        d.todos = keep;
        d.trash.extend(done);
        if d.trash.len() > 50 {
            let overflow = d.trash.len() - 50;
            d.trash.drain(0..overflow);
        }
        d.clone()
    };
    persist(&app, &data)?;
    Ok(data)
}

/// 拖拽排序：传入按新顺序排列的 id 列表
#[tauri::command]
pub fn reorder_todos(app: AppHandle, ids: Vec<String>) -> Result<AppData, String> {
    let state = app.state::<AppState>();
    let data = {
        let mut d = state.data.lock().unwrap();
        for (i, id) in ids.iter().enumerate() {
            if let Some(t) = d.todos.iter_mut().find(|t| &t.id == id) {
                t.order = (i as i64 + 1) * 10;
            }
        }
        crate::model::normalize(&mut d.todos);
        d.clone()
    };
    persist(&app, &data)?;
    Ok(data)
}

/// 稍后提醒：延后 N 分钟并清空已提醒标记
#[tauri::command]
pub fn snooze_todo(app: AppHandle, id: String, minutes: i64) -> Result<AppData, String> {
    let state = app.state::<AppState>();
    let data = {
        let mut d = state.data.lock().unwrap();
        let now = chrono::Local::now();
        let Some(t) = d.todos.iter_mut().find(|t| t.id == id) else {
            return Err("待办不存在".into());
        };
        let base = t
            .due_at
            .as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Local))
            .filter(|dt| *dt > now)
            .unwrap_or(now);
        t.due_at = Some((base + chrono::Duration::minutes(minutes)).to_rfc3339());
        t.notified_at = None;
        t.updated_at = now.to_rfc3339();
        d.clone()
    };
    persist(&app, &data)?;
    Ok(data)
}

// ---------------------------------------------------------------- 设置

#[tauri::command]
pub fn save_settings(app: AppHandle, settings: Settings) -> Result<AppData, String> {
    let state = app.state::<AppState>();
    let data = {
        let mut d = state.data.lock().unwrap();
        let old_sync = d.settings.sync.clone();
        d.settings = settings;
        // 同步配置变化时清掉脏标记，避免误提交
        if old_sync.remote_url != d.settings.sync.remote_url {
            d.settings.sync.token = d.settings.sync.token.trim().to_string();
        }
        d.clone()
    };
    persist(&app, &data)?;

    // 主题与置顶立即生效
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.set_always_on_top(data.settings.window.always_on_top);
    }
    Ok(data)
}

/// 收起/展开：同时调整窗口高度并记忆状态
#[tauri::command]
pub fn set_collapsed(app: AppHandle, collapsed: bool) -> Result<AppData, String> {
    let state = app.state::<AppState>();
    let (data, expanded) = {
        let mut d = state.data.lock().unwrap();
        d.settings.window.collapsed = collapsed;
        let (w, h) = d.settings.window.size.unwrap_or((320.0, 460.0));
        (d.clone(), (w.max(260.0), h.max(crate::model::MIN_EXPANDED_HEIGHT)))
    };
    if let Some(win) = app.get_webview_window("main") {
        let (w, h) = expanded;
        if collapsed {
            // 先锁定不可调整大小，避免收起高度被 resize 事件写回记忆
            let _ = win.set_resizable(false);
            let _ = win.set_size(tauri::LogicalSize::new(w, crate::model::COLLAPSED_HEIGHT));
        } else {
            let _ = win.set_size(tauri::LogicalSize::new(w, h));
            let _ = win.set_resizable(true);
        }
    }
    persist(&app, &data)?;
    Ok(data)
}

/// 拖动窗口：前端在拖拽标题栏时调用
#[tauri::command]
pub fn move_window(app: AppHandle, dx: f64, dy: f64) -> Result<(), String> {
    let Some(win) = app.get_webview_window("main") else {
        return Err("窗口不存在".into());
    };
    let pos = win.outer_position().map_err(|e| e.to_string())?;
    let scale = win.scale_factor().unwrap_or(1.0);
    let nx = pos.x + (dx * scale).round() as i32;
    let ny = pos.y + (dy * scale).round() as i32;
    win.set_position(tauri::PhysicalPosition::new(nx, ny))
        .map_err(|e| e.to_string())
}

/// 前端解析好目标位置后一次性写入（避免累积误差）
#[tauri::command]
pub fn save_window_state(
    app: AppHandle,
    x: Option<i32>,
    y: Option<i32>,
    width: Option<f64>,
    height: Option<f64>,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let data = {
        let mut d = state.data.lock().unwrap();
        if let (Some(x), Some(y)) = (x, y) {
            d.settings.window.position = Some((x, y));
        }
        if let (Some(w), Some(h)) = (width, height) {
            d.settings.window.size = Some((w, h));
        }
        d.clone()
    };
    persist(&app, &data)
}

#[tauri::command]
pub fn hide_window(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("main") {
        win.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn show_window(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.set_always_on_top(true);
        let _ = win.set_focus();
    }
    Ok(())
}

/// 把窗口移回主屏右上角，并清掉记忆的位置
#[tauri::command]
pub fn reposition_window(app: AppHandle) -> Result<(), String> {
    let win = app
        .get_webview_window("main")
        .ok_or_else(|| "窗口不存在".to_string())?;

    let (mut x, mut y) = (60i32, 60i32);
    if let Ok(Some(mon)) = win.primary_monitor() {
        let msize = mon.size();
        let mpos = mon.position();
        let wsize = win.outer_size().map_err(|e| e.to_string())?;
        let margin = (24.0 * win.scale_factor().unwrap_or(1.0)) as i32;
        x = mpos.x + msize.width as i32 - wsize.width as i32 - margin;
        y = mpos.y + (48.0 * win.scale_factor().unwrap_or(1.0)) as i32;
    }
    win.set_position(tauri::PhysicalPosition::new(x.max(0), y.max(0)))
        .map_err(|e| e.to_string())?;
    let _ = win.show();
    let _ = win.set_focus();

    let state = app.state::<AppState>();
    let data = {
        let mut d = state.data.lock().unwrap();
        d.settings.window.position = Some((x.max(0), y.max(0)));
        d.clone()
    };
    persist(&app, &data)
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}

// ---------------------------------------------------------------- 导入导出

#[tauri::command]
pub async fn export_data(app: AppHandle) -> Result<Option<String>, String> {
    let default_name = format!(
        "desk-todo-{}.json",
        chrono::Local::now().format("%Y%m%d-%H%M")
    );
    let picked = tauri_plugin_dialog::DialogExt::dialog(&app)
        .file()
        .set_title("导出待办数据")
        .set_file_name(&default_name)
        .add_filter("JSON", &["json"])
        .blocking_save_file();
    let Some(path) = picked else {
        return Ok(None);
    };
    let path = path
        .into_path()
        .map_err(|e| format!("无法解析保存路径: {e}"))?;
    let data = snapshot(&app);
    crate::store::export_to(&std::path::PathBuf::new(), &data, &path)?;
    if let Some(s) = path.to_str() {
        let _ = app.emit("data://exported", s.to_string());
    }
    Ok(Some(path.display().to_string()))
}

#[derive(Debug, Serialize)]
pub struct ImportResult {
    pub added: usize,
    pub total: usize,
}

#[tauri::command]
pub async fn import_data(app: AppHandle, merge: Option<bool>) -> Result<Option<ImportResult>, String> {
    let picked = tauri_plugin_dialog::DialogExt::dialog(&app)
        .file()
        .set_title("导入待办数据")
        .add_filter("JSON", &["json"])
        .blocking_pick_file();
    let Some(path) = picked else {
        return Ok(None);
    };
    let path = path
        .into_path()
        .map_err(|e| format!("无法解析文件路径: {e}"))?;
    let incoming = crate::store::import_from(&path)?;
    let merge = merge.unwrap_or(true);

    let state = app.state::<AppState>();
    let (data, added) = {
        let mut d = state.data.lock().unwrap();
        if merge {
            let existing: std::collections::HashSet<String> =
                d.todos.iter().map(|t| t.id.clone()).collect();
            let mut added = 0usize;
            for mut t in incoming.todos {
                if existing.contains(&t.id) {
                    // id 冲突时当作新条目
                    t.id = uuid::Uuid::new_v4().to_string();
                }
                d.todos.push(t);
                added += 1;
            }
            crate::model::normalize(&mut d.todos);
            (d.clone(), added)
        } else {
            let mut fresh = incoming.clone();
            fresh.settings = d.settings.clone();
            *d = fresh;
            let total = d.todos.len();
            (d.clone(), total)
        }
    };
    persist(&app, &data)?;
    Ok(Some(ImportResult {
        added,
        total: data.todos.len(),
    }))
}

// ---------------------------------------------------------------- 通知 / 自启

pub fn send_notification(app: &AppHandle, title: &str, body: &str) {
    if let Err(e) = app
        .notification()
        .builder()
        .title(title)
        .body(body)
        .show()
    {
        eprintln!("[notify] 发送通知失败: {e}");
    }
}

#[tauri::command]
pub fn notify_now(app: AppHandle, title: String, body: String) {
    send_notification(&app, &title, &body);
}

#[tauri::command]
pub fn set_autostart(app: AppHandle, enabled: bool) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    if enabled {
        manager.enable().map_err(|e| e.to_string())?;
    } else {
        manager.disable().map_err(|e| e.to_string())?;
    }
    manager.is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_autostart(app: AppHandle) -> bool {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().unwrap_or(false)
}
