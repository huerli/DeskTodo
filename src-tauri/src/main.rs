// Windows 下隐藏控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if std::env::var("DESK_TODO_SSH_ASKPASS").as_deref() == Ok("1") {
        std::process::exit(desk_todo_lib::ssh_askpass_main());
    }
    // 无界面自检：验证存储 / 到期判定 / Git 同步
    if std::env::args().any(|a| a == "--self-test") {
        desk_todo_lib::self_test::run_headless();
        return;
    }
    desk_todo_lib::run()
}
