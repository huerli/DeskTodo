# DeskTodo · 常驻桌面的待办小窗

一个始终置顶、可拖拽收起的桌面待办清单。三平台通用（macOS / Windows / Linux），数据保存在本地，
可选同步到自己的 Git 仓库。基于 **Tauri v2 + Rust**，安装包体积小、内存占用低。

```
┌──────────────────────────────┐
│ ⌄  待办            ③   –  ⚙  │  ← 按住这里拖动窗口
├──────────────────────────────┤
│ 5 项待办 · 今天 2 项 · 1 项逾期│
│ ⠿ ○ 写周报          ⏰ 2 小时后 │  ← 拖 ⠿ 排序
│ ⠿ ○ 交房租          ⏰ 3 天后   │
│ ⠿ ● 买咖啡          ✓         │
├──────────────────────────────┤
│ [ 添加待办，回车保存… ] [时间] +│
│  低 普通 高                    │
└──────────────────────────────┘
```

---

## 功能

| 类别 | 能力 |
| --- | --- |
| 待办管理 | 新增 / 行内改名 / 勾选完成 / 删除 / 撤销删除（⌘Z、Ctrl+Z）/ 清除已完成 |
| 排序 | 手动拖拽排序，或按到期时间 / 优先级 / 创建时间自动排序；已完成项自动沉底 |
| 到期提醒 | 到期（或提前 1 分钟 ~ 1 天）弹系统通知；可选提示音与语音播报；可设提前量 |
| 桌面常驻 | 窗口始终置顶、无边框、可拖动、可收缩成一条细栏；关闭按钮 = 隐藏到托盘（不退出） |
| 托盘 | 左键点图标显示 / 隐藏；菜单显示待办摘要、快速新增、立即同步、退出 |
| 开机自启 | 设置面板一键开关（macOS LaunchAgent / Windows 注册表 / Linux .desktop） |
| 数据 | 本地 `todos.json` 原子写入；`backups/` 每小时自动留档（保留 12 份）；导出 / 导入 JSON |
| Git 同步 | 数据目录就是一个 git 仓库，可推送到 GitHub / GitLab / 自建 Gitea；支持自动提交与自动同步 |
| 界面 | 深色 / 浅色 / 跟随系统；不透明度可调；⌘N 聚焦输入框 |

---

## 一、快速开始

### 1. 安装 Rust 工具链（仅首次）

macOS（本机沙箱不允许写 `~/.cargo`，脚本会把工具链装到工作区内的 `.rust-toolchain/`）：

```bash
bash scripts/bootstrap-macos.sh
```

Windows / Linux 直接用官方安装器即可：

```bash
# Windows (PowerShell)：winget install Rustlang.Rustup
# Linux / macOS 标准方式：
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### 2. 构建并运行

```bash
source scripts/env.sh          # 注入 cargo / node 环境
bash scripts/build.sh release app   # release 构建，出 .app
bash scripts/run.sh            # 直接启动（免安装）
```

想做成可安装的安装包（macOS）：

```bash
bash scripts/make-dmg.sh       # 生成 dist/DeskTodo_<版本>_<架构>.dmg
```

首次构建需要编译 Tauri 依赖，约 3–8 分钟；之后增量构建在 10 秒级。

> 注意：前端资源是**编译期内嵌**的。改了 `src/` 下的文件必须重新 `build`，只重启进程无效。

### 3. 开发模式（热重载前端）

前端是纯静态文件（`src/index.html`、`src/styles.css`、`src/app.js`），改完前端 **重新构建并重启应用** 即生效，
不需要打包工具。需要改窗口配置（`src-tauri/tauri.conf.json`）时：

```bash
source scripts/env.sh
cargo tauri dev            # 需要先 cargo install tauri-cli --version '^2.0' --locked
```

---

## 二、三平台打包

### 当前平台

```bash
bash scripts/build.sh release            # 使用 tauri.conf.json 里的默认 bundle 目标
bash scripts/build.sh release app,dmg    # macOS：只出 .app 和 .dmg
bash scripts/build.sh release nsis,msi   # Windows
bash scripts/build.sh release appimage,deb  # Linux
```

产物位置：`src-tauri/target/release/bundle/<类型>/`（若设置了 `CARGO_TARGET_DIR` 则为
`.cargo-target/release/bundle/<类型>/`）。

### 生成 macOS 安装包（推荐）

```bash
bash scripts/build.sh release app     # 1) 先出 .app
bash scripts/make-dmg.sh              # 2) 打成可安装的 DMG
```

`make-dmg.sh` 生成的 DMG 里包含：`DeskTodo.app`、指向「应用程序」的快捷方式、
以及一份 `安装说明.txt`。它会自动挂载校验内容后再卸载，产物落在 `dist/`。

> 与 `tauri build --bundles dmg` 的区别：后者生成的 DMG 只有 .app 本身，
> 用户需要自己把 App 拖到「应用程序」；本脚本额外放了快捷方式和说明。

### 分发给别人之前（重要）

本机构建的 `.app` **没有签名**，别人下载后 macOS 会拦下。接收方需要：

```bash
xattr -dr com.apple.quarantine "/Applications/DeskTodo.app"
```

或者右键点 App → 打开 → 再点「打开」。要彻底免除这一步，需要 Apple Developer
账号做签名与公证（在 `tauri.conf.json` 的 `bundle.macOS` 里配置 `signingIdentity`
与 `notarize`）。

| 平台 | 产物 | 系统依赖 |
| --- | --- | --- |
| macOS | `.app` / `.dmg` | Xcode Command Line Tools |
| Windows | `-setup.exe`（NSIS）/ `.msi` | WebView2 运行时（Win10+ 通常已内置）、MSVC 生成工具 |
| Linux | `.AppImage` / `.deb` | `libwebkit2gtk-4.1-dev`、`libayatana-appindicator3-dev`、`librsvg2-dev`、`patchelf`、`libssl-dev` |

### 三平台一次出包（CI）

推送 `v*` 标签即可触发 `.github/workflows/build.yml`，在 GitHub Actions 上并行构建
macOS(arm64/x64)、Linux、Windows 安装包并创建 Draft Release：

```bash
git tag v0.1.0 && git push origin v0.1.0
```

> 交叉编译不可行：Tauri 必须在目标操作系统上打包。要在本机产出 Windows 安装包，
> 需在 Windows 上运行 `scripts/build.sh`，或使用上面的 CI。

---

## 三、Git 同步

数据目录同时是一个 git 仓库，把待办历史版本化并同步到自己的私有仓库。

1. 在 GitHub / GitLab 新建一个 **私有空仓库**（不要勾选初始化 README）。
2. 应用内 `⚙ → Git 同步`：
   - **远程地址**：`git@github.com:you/todo-data.git`（SSH，推荐）或 `https://github.com/you/todo-data.git`
   - **远程名 / 分支**：`origin` / `main`
   - **提交者**：填写 name 与 email（git 提交需要）
   - HTTPS 私有仓库：再填 **用户名 + 访问令牌**（Personal Access Token，不是登录密码）
   - SSH：本机已有 key 时无需填写；需要指定私钥则填 `~/.ssh/id_ed25519`
3. 点 **测试连接** → **初始化/提交** → **立即同步**。

实现方式（有意分两层，兼顾稳定与安全）：

- 本地操作（init / status / 暂存 / 提交 / 历史比较）由内置 **libgit2** 完成，离线可用；
- 网络操作（fetch / pull / push）调用**系统 `git`**，因此直接复用你已配置好的 SSH key、
  ssh-agent、macOS Keychain / Windows 凭据管理器；令牌通过临时 credential helper 注入，
  **不会写进 `.git/config`**。

同步流程：本地提交 → fetch → `rebase --autostash` → push。
遇到冲突会 **中止变基并保留本地数据**，提示你手动处理，不会静默覆盖任何一边。

也可在终端直接操作：

```bash
cd "$(系统应用数据目录)/com.desktodo.app"   # 应用内 ⚙→数据 面板可查看确切路径
git log --oneline            # 查看待办历史
git diff HEAD~1              # 看上次改了什么
git checkout HEAD~3 -- todos.json   # 回滚到 3 个提交前
```

---

## 四、数据与备份

| 位置 | 内容 |
| --- | --- |
| `todos.json` | 全部待办与设置（原子写入：先写临时文件再 rename） |
| `backups/YYYYmmdd-HHMMSS.json` | 每小时自动留档，最多 12 份，可手工回滚 |
| `todos.corrupted-*.json` | 若数据文件损坏，原文件会被改名留档，并自动尝试从最近备份恢复 |

数据目录（应用内 `⚙ → 数据` 可直接打开）：

- macOS: `~/Library/Application Support/com.desktodo.app/`
- Windows: `%APPDATA%\com.desktodo.app\`
- Linux: `~/.local/share/com.desktodo.app/`

`todos.json` 结构（版本化，向后兼容读取）：

```json
{
  "version": 1,
  "todos": [
    {
      "id": "uuid",
      "title": "写周报",
      "notes": "",
      "done": false,
      "due_at": "2026-03-05T14:30:00+08:00",
      "priority": "normal",
      "tags": ["工作"],
      "created_at": "2026-03-05T09:00:00+08:00",
      "updated_at": "2026-03-05T09:00:00+08:00",
      "completed_at": null,
      "notified_at": null,
      "order": 10
    }
  ],
  "settings": { "window": {}, "reminder": {}, "sync": {}, "sort": "manual", "start_hidden": false },
  "trash": [],
  "last_sync": null
}
```

---

## 五、快捷键与交互

| 操作 | 方式 |
| --- | --- |
| 添加待办 | 输入框回车（可同时选到期时间与优先级） |
| 改名 | 直接点标题编辑，回车保存，Esc 取消 |
| 完成 / 取消完成 | 点左侧圆圈 |
| 拖拽排序 | 悬停条目左侧出现 ⠿，按住拖动（需排序方式为「手动」） |
| 撤销删除 | ⌘Z / Ctrl+Z |
| 聚焦输入框 | ⌘N / Ctrl+N |
| 拖动窗口 | 按住标题栏空白处拖动 |
| 收起 / 展开 | 点标题栏左侧箭头（收起后窗口自缩成一条细栏） |
| 隐藏到托盘 | 点标题栏 `–`；关闭窗口按钮同样是隐藏 |
| 退出应用 | 托盘菜单 → 退出 DeskTodo |

---

## 六、自检与诊断

### 无人值守自检

不需要图形界面即可验证核心逻辑（存储、备份自愈、CRUD、排序、导出导入、
到期判定、Git 本地提交与真实 push/pull）：

```bash
bash scripts/build.sh debug
.bundle 或 target 下的 desk-todo --self-test
# 或
CARGO_TARGET_DIR=../.cargo-target src-tauri/target/debug/desk-todo --self-test
```

输出示例（节选）：

```
[3/6] 落盘 + 重读一致性
  ok   导出 JSON 文件
  ok   导入后标题/优先级/到期时间一致
[6/6] Git 推送 / 拉取（本地裸仓库作为远程）
  ok   已推送 (已获取远程更新；已推送到远程)
  ok   拉取后本地已包含另一台设备的数据
== 全部通过 ==
```

### 冒烟启动

验证 GUI 能否正常创建窗口与托盘，N 秒后自动退出：

```bash
bash scripts/run.sh debug --exit-after 8
```

### 诊断文件

应用数据目录下会自动写入两个诊断文件，界面出问题时先看它们：

| 文件 | 内容 |
| --- | --- |
| `boot-trace.log` | 前端启动轨迹（每一步耗时），用于定位"界面空白"卡在哪一步 |
| `frontend.log` | 前端 `console.error`、未捕获异常、未处理的 Promise 拒绝 |

### 排查经验（开发时容易踩的坑）

1. **前端资源是编译期内嵌的**：改完 `src/` 下的文件必须重新 `build`，重启进程没用。
2. **WKWebView 会持久缓存静态资源**：如果改了前端却看不到效果，执行
   `rm -rf ~/Library/WebKit/<bundle-id> ~/Library/Caches/<bundle-id>` 后重试。
3. **`window.prompt()` 在 WKWebView 中不存在**：本项目用自绘弹窗（`askBox`）代替。
4. **透明窗口需要 macOS 私有 API**：本项目改用不透明窗口 + CSS 圆角，避免 App Store 拒审。

---

## 七、项目结构

```
desk-todo/
├─ src/                        前端（纯静态，无构建步骤）
│  ├─ index.html               结构：标题栏 / 列表 / 新增行 / 设置面板
│  ├─ styles.css               深色·浅色主题，圆角玻璃质感
│  └─ app.js                   Tauri 命令调用、渲染、拖拽、通知与语音
├─ src-tauri/
│  ├─ src/lib.rs               应用装配、到期扫描线程、自动提交线程、窗口事件
│  ├─ src/main.rs              入口（含 --self-test / --exit-after 参数）
│  ├─ src/model.rs             数据模型与磁盘格式
│  ├─ src/store.rs             原子写入、备份轮转、损坏自愈、导入导出
│  ├─ src/commands.rs          Tauri 命令：CRUD / 设置 / 导入导出 / 自启 / 通知 / 诊断
│  ├─ src/git_sync.rs          Git 同步（libgit2 + 系统 git 两层）
│  ├─ src/tray.rs              托盘图标与菜单
│  ├─ src/self_test.rs         无人值守自检
│  ├─ Cargo.toml               依赖与 release 优化（LTO / strip）
│  ├─ tauri.conf.json          窗口与打包配置
│  ├─ build.rs                 构建脚本
│  └─ capabilities/default.json 权限白名单
├─ scripts/
│  ├─ env.sh                   环境注入（cargo / node）
│  ├─ bootstrap-macos.sh       安装 Rust 工具链到工作区内
│  ├─ make_icon.py             程序化生成图标源图
│  ├─ gen_icons.py             派生 PNG / ICO / ICNS
│  ├─ icons.sh                 一键重新生成图标
│  ├─ build.sh                 构建 + 打包
│  ├─ make-dmg.sh              生成带「应用程序」快捷方式的 DMG
│  └─ run.sh                   启动
└─ .github/workflows/build.yml 三平台 CI 打包
```

---

## 八、常见问题

**收不到系统通知？**
`⚙ → 通用 → 发送一条测试通知`。macOS 需在「系统设置 → 通知」中允许 DeskTodo；
Windows 需确保「专注助手」未屏蔽；Linux 需要 `libnotify`（GNOME/KDE 自带）。

**首次打开提示「已损坏」或无法打开（macOS）？**
本机自构建的 `.app` 未签名/未公证时，系统会拦截。解决：

```bash
xattr -dr com.apple.quarantine "/Applications/DeskTodo.app"
```

要分发给别人，需要 Apple Developer 账号做签名与公证（在 `tauri.conf.json` 的
`bundle.macOS` 中配置签名身份）。

**窗口在某个屏幕上不见了？**
`⚙ → 窗口 → 把窗口移回屏幕右上角`，或删除 `todos.json` 里的 `settings.window.position`。

**Git 推送失败？**
- `could not read Username`：HTTPS 仓库需要填用户名 + 访问令牌，或改用 SSH 地址。
- `Permission denied (publickey)`：先用 `ssh -T git@github.com` 确认本地 SSH key 可用。
- 提示存在冲突：应用已中止自动变基、本地数据完好。在数据目录手动 `git status` 处理后重试。

**托盘图标在 Linux 上不显示？**
需要 AppIndicator 支持（`libayatana-appindicator3-dev`），GNOME 还需安装
`gnome-shell-extension-appindicator`。

---

## 九、许可

个人使用自由；如需商用请自行确认内置依赖（Tauri 为 MIT/Apache-2.0，libgit2 为 GPL-2.0 with linking exception）的合规性。
