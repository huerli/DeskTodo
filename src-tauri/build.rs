/// 构建脚本：生成权限 schema、嵌入前端资源。
///
/// 注意：前端资源（`../src`）由 Tauri 在**编译期**嵌入二进制。
/// 因此修改 HTML/CSS/JS 后必须重新构建（`bash scripts/build.sh`）才会生效；
/// 仅重启进程不会加载新前端。
fn main() {
    // 前端文件变化时触发重新构建
    println!("cargo:rerun-if-changed=../src/index.html");
    println!("cargo:rerun-if-changed=../src/app.js");
    println!("cargo:rerun-if-changed=../src/styles.css");

    tauri_build::build()
}
