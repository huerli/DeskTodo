#!/usr/bin/env bash
# 生成 macOS 安装包（DMG）
#
# 与 `tauri build --bundles dmg` 的区别：本脚本在 DMG 内额外放一个
# 「应用程序」文件夹快捷方式，用户挂载后直接拖进去即可（标准 macOS 安装姿势），
# 并附带一份安装说明。
#
# 用法：
#   bash scripts/make-dmg.sh            # 需要先 bash scripts/build.sh release app
#
# 产物：dist/DeskTodo_<版本>_<架构>.dmg
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
# shellcheck disable=SC1091
source scripts/env.sh

APP_NAME="DeskTodo"
VOL_NAME="DeskTodo"
VERSION="$(python3 -c "import json;print(json.load(open('src-tauri/tauri.conf.json'))['version'])" 2>/dev/null || echo 0.1.0)"
ARCH="$(uname -m)"
case "$ARCH" in
  arm64) ARCH_LABEL="aarch64" ;;
  x86_64) ARCH_LABEL="x64" ;;
  *) ARCH_LABEL="$ARCH" ;;
esac

APP_DIR="${CARGO_TARGET_DIR:-$ROOT/src-tauri/target}/release/bundle/macos/$APP_NAME.app"
if [ ! -d "$APP_DIR" ]; then
  echo "未找到 $APP_DIR"
  echo "请先执行： bash scripts/build.sh release app"
  exit 1
fi

OUT_DIR="$ROOT/dist"
mkdir -p "$OUT_DIR"
DMG_PATH="$OUT_DIR/${APP_NAME}_${VERSION}_${ARCH_LABEL}.dmg"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

echo "==> 准备 DMG 内容"
cp -R "$APP_DIR" "$STAGE/"
# 「应用程序」快捷方式：挂载后可直接把 App 拖进去
ln -s /Applications "$STAGE/应用程序"

cat > "$STAGE/安装说明.txt" <<'TXT'
DeskTodo 安装说明
=================

安装
----
把左边的 DeskTodo 拖到右边的「应用程序」文件夹即可。

首次打开被系统拦下？
--------------------
本安装包没有 Apple 开发者签名，macOS 可能提示「已损坏」或「无法验证开发者」。
任选一种方式解决：

  1) 在「应用程序」里右键点 DeskTodo → 打开 → 再点「打开」；
  2) 或执行一次：
       xattr -dr com.apple.quarantine "/Applications/DeskTodo.app"

之后就能正常双击启动了。

第一次使用
----------
* 窗口会贴在屏幕右上角并始终置顶，按住标题栏空白处可拖动。
* 关闭窗口 = 隐藏到托盘（不退出）；退出请用菜单栏托盘图标 → 退出。
* 开机自启、主题、提醒提前量等都在右上角齿轮里设置。
* 到期提醒需要系统通知权限，可在设置里点「发送一条测试通知」验证。

数据在哪里
----------
~/Library/Application Support/com.desktodo.app/todos.json
（原子写入，另有 backups/ 目录每小时留一份，可在应用内「设置 → 数据」打开该目录）

Git 同步
--------
设置 → Git 同步：填一个私有仓库地址即可把待办历史版本化。
SSH 地址直接复用你本机已有的 key；HTTPS 需填用户名与访问令牌。
TXT

echo "==> 生成 DMG: $(basename "$DMG_PATH")"
rm -f "$DMG_PATH"
hdiutil create \
  -volname "$VOL_NAME" \
  -srcfolder "$STAGE" \
  -ov -format UDZO \
  "$DMG_PATH" >/dev/null

echo "==> 校验 DMG 可挂载"
MOUNT_POINT="$(hdiutil attach "$DMG_PATH" -nobrowse -readonly | tail -1 | awk '{$1="";$2="";print substr($0,3)}')"
if [ -z "$MOUNT_POINT" ] || [ ! -d "$MOUNT_POINT" ]; then
  echo "挂载失败"; exit 1
fi
echo "  挂载点: $MOUNT_POINT"
ls -1 "$MOUNT_POINT" | sed 's/^/    /'
if [ -x "$MOUNT_POINT/$APP_NAME.app/Contents/MacOS/desk-todo" ]; then
  echo "  ✓ 内含可执行文件"
else
  echo "  ✗ 可执行文件缺失"; hdiutil detach "$MOUNT_POINT" >/dev/null; exit 1
fi
hdiutil detach "$MOUNT_POINT" >/dev/null
echo "  已卸载"

echo
echo "==> 完成"
du -sh "$DMG_PATH" | sed 's/^/  /'
