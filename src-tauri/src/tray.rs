//! 系统托盘：显示/隐藏、快速添加、退出；菜单摘要实时显示未完成数量

use crate::AppState;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use std::sync::Mutex;

const TRAY_ID: &str = "desk-todo-tray";
const ICON_BYTES: &[u8] = include_bytes!("../icons/32x32.png");

/// 持有托盘菜单中「摘要」项的句柄，便于后续更新文案
pub struct TrayHandle<R: Runtime> {
    summary: Mutex<MenuItem<R>>,
}

impl<R: Runtime> TrayHandle<R> {
    fn set_summary(&self, text: &str) {
        if let Ok(item) = self.summary.lock() {
            let _ = item.set_text(text);
        }
    }
}

pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let (done, total, overdue) = counts(app);
    let title = summary(done, total, overdue);

    let summary_item = MenuItem::with_id(app, "summary", &title, false, None::<&str>)?;
    let show_item = MenuItem::with_id(app, "toggle", "显示 / 隐藏窗口", true, None::<&str>)?;
    let add_item = MenuItem::with_id(app, "add", "新增待办…", true, None::<&str>)?;
    let sync_item = MenuItem::with_id(app, "sync", "立即同步到 Git", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "退出 DeskTodo", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;

    let menu = Menu::with_items(
        app,
        &[
            &summary_item,
            &sep1,
            &show_item,
            &add_item,
            &sync_item,
            &sep2,
            &quit_item,
        ],
    )?;

    // 保存句柄，供 refresh 更新文案
    app.manage(TrayHandle {
        summary: Mutex::new(summary_item),
    });

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(load_icon(app))
        .icon_as_template(true)
        .tooltip(&title)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "toggle" => toggle_window(app),
            "add" => {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.set_always_on_top(true);
                    let _ = win.set_focus();
                }
                let _ = app.emit("ui://focus-input", ());
            }
            "sync" => {
                let _ = app.emit("ui://sync-request", ());
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_window(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

/// 刷新托盘提示与菜单摘要
pub fn refresh<R: Runtime>(app: &AppHandle<R>) {
    let (done, total, overdue) = counts(app);
    let title = summary(done, total, overdue);
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(&title));
    }
    if let Some(handle) = app.try_state::<TrayHandle<R>>() {
        handle.set_summary(&title);
    }
}

fn counts<R: Runtime>(app: &AppHandle<R>) -> (usize, usize, usize) {
    let Some(state) = app.try_state::<AppState>() else {
        return (0, 0, 0);
    };
    let d = state.data.lock().unwrap();
    let total = d.todos.len();
    let done = d.todos.iter().filter(|t| t.done).count();
    let now = chrono::Local::now();
    let overdue = d
        .todos
        .iter()
        .filter(|t| {
            !t.done
                && t.due_at
                    .as_ref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&chrono::Local) < now)
                    .unwrap_or(false)
        })
        .count();
    (done, total, overdue)
}

fn summary(done: usize, total: usize, overdue: usize) -> String {
    if total == 0 {
        return "DeskTodo · 暂无待办".into();
    }
    let pending = total - done;
    if overdue > 0 {
        format!("DeskTodo · 待办 {pending} 项（{overdue} 项已逾期）")
    } else {
        format!("DeskTodo · 待办 {pending} 项，已完成 {done} 项")
    }
}

fn toggle_window<R: Runtime>(app: &AppHandle<R>) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    match win.is_visible() {
        Ok(true) => {
            let _ = win.hide();
        }
        _ => {
            let _ = win.show();
            let _ = win.set_always_on_top(true);
            let _ = win.set_focus();
        }
    }
}

fn load_icon<R: Runtime>(app: &AppHandle<R>) -> tauri::image::Image<'static> {
    if let Ok(img) = image::load_from_memory(ICON_BYTES) {
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        return tauri::image::Image::new_owned(rgba.into_raw(), w, h);
    }
    // 退路：把默认窗口图标复制成自有数据（避免借用生命周期）
    match app.default_window_icon() {
        Some(icon) => {
            let (w, h) = (icon.width(), icon.height());
            tauri::image::Image::new_owned(icon.rgba().to_vec(), w, h)
        }
        None => tauri::image::Image::new_owned(vec![0u8; 4], 1, 1),
    }
}
