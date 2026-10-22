#!/usr/bin/env bash
# 本地构建：构建当前平台的可执行文件与安装包
#
#   bash scripts/build.sh             # release 构建（当前平台默认 bundle）
#   bash scripts/build.sh debug       # debug 构建，不打包
#   bash scripts/build.sh fast        # 快速优化构建，不打包
#   bash scripts/build.sh release app # macOS：只出 .app
#   bash scripts/build.sh release app,dmg   # macOS
#   bash scripts/build.sh release nsis,msi  # Windows
#   bash scripts/build.sh release appimage,deb  # Linux
#
# 依赖：scripts/env.sh 中的 Rust 工具链；Linux 需要 webkit2gtk 等系统库
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
# shellcheck disable=SC1091
source scripts/env.sh

# 安装包与程序版本必须一致，防止只更新 package.json 后误发旧版本。
node scripts/check-version.mjs

MODE="${1:-release}"
BUNDLES=""
shift "$(( $# > 0 ? 1 : 0 ))"
if [ "${1:-}" = "--bundles" ]; then
  shift
  if [ -z "${1:-}" ]; then
    echo "--bundles 后需要目标，例如：bash scripts/build.sh release --bundles deb" >&2
    exit 1
  fi
fi
if [ "$#" -gt 1 ]; then
  echo "用法: bash scripts/build.sh [debug|fast|release] [--bundles] [逗号分隔的 bundle 列表]" >&2
  exit 1
fi
BUNDLES="${1:-}"
case "$MODE" in
  debug|fast|release) ;;
  *) echo "构建模式必须为 debug、fast 或 release" >&2; exit 1 ;;
esac
if [ "$MODE" != release ] && [ -n "$BUNDLES" ]; then
  echo "$MODE 模式不打包，请移除 bundle 参数" >&2
  exit 1
fi

PLATFORM="$(uname -s)"
case "$PLATFORM" in
  Linux) ALLOWED="deb rpm appimage" ;;
  Darwin) ALLOWED="app dmg" ;;
  MINGW*|MSYS*|CYGWIN*) ALLOWED="nsis msi" ;;
  *) ALLOWED="" ;;
esac
if [ -n "$BUNDLES" ] && [ -n "$ALLOWED" ]; then
  IFS=',' read -r -a TARGETS <<< "$BUNDLES"
  for target in "${TARGETS[@]}"; do
    case " $ALLOWED " in
      *" $target "*) ;;
      *) echo "$PLATFORM 不支持 bundle '$target'，可选目标：$ALLOWED" >&2; exit 1 ;;
    esac
  done
fi

# 提前报告系统依赖，避免 Cargo 输出大量重复的 pkg-config 错误。
if [ "$PLATFORM" = Linux ]; then
  MISSING=()
  for module in 'glib-2.0 >= 2.70' 'gobject-2.0 >= 2.70' gio-2.0 gtk+-3.0 webkit2gtk-4.1 ayatana-appindicator3-0.1 librsvg-2.0 dbus-1; do
    if ! pkg-config --exists "$module" 2>/dev/null; then
      MISSING+=("$module")
    fi
  done
  if [ "${#MISSING[@]}" -gt 0 ]; then
    echo "缺少 Linux 编译依赖（或版本不足）：${MISSING[*]}" >&2
    echo "Ubuntu / Debian 请先执行：" >&2
    echo "  sudo apt-get update" >&2
    echo "  sudo apt-get install -y --no-install-recommends build-essential pkg-config libssl-dev libgtk-3-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libdbus-1-dev libxdo-dev patchelf" >&2
    exit 1
  fi
fi

command -v cargo >/dev/null || {
  echo "未找到 cargo，请先执行： bash scripts/bootstrap-macos.sh"
  exit 1
}

# tauri CLI：优先 Rust 版 cargo-tauri（自包含，不依赖 Node 原生模块）
TAURI_BIN=""
if command -v cargo-tauri >/dev/null; then
  TAURI_BIN="$(command -v cargo-tauri)"
elif [ -n "${CARGO_HOME:-}" ] && [ -x "$CARGO_HOME/bin/cargo-tauri" ]; then
  TAURI_BIN="$CARGO_HOME/bin/cargo-tauri"
elif [ -x "$ROOT/node_modules/.bin/tauri" ]; then
  TAURI_BIN="$ROOT/node_modules/.bin/tauri"
fi

case "$MODE" in
  fast)
    echo "==> fast 构建（增量编译，不打包）"
    cargo build --manifest-path src-tauri/Cargo.toml --profile fast --features custom-protocol
    echo "==> 可执行文件: ${CARGO_TARGET_DIR:-src-tauri/target}/fast/desk-todo"
    exit 0
    ;;

  debug)
    echo "==> debug 构建（不打包）"
    cargo build --manifest-path src-tauri/Cargo.toml
    echo "==> 可执行文件: ${CARGO_TARGET_DIR:-src-tauri/target}/debug/desk-todo"
    exit 0
    ;;

  release)
    [ -n "$TAURI_BIN" ] || {
      echo "未找到 tauri CLI。安装方式："
      echo "  cargo install tauri-cli --version '^2.0' --locked"
      exit 1
    }
    # cargo-tauri 需要在 src-tauri 目录下运行（不支持 --manifest-path）
    cd "$ROOT/src-tauri"
    ARGS=(build)
    [ -n "$BUNDLES" ] && ARGS+=(--bundles "$BUNDLES")
    echo "==> release 构建 + 打包 ${BUNDLES:-默认目标}"
    "$TAURI_BIN" "${ARGS[@]}"
    ;;

  *)
    echo "用法: bash scripts/build.sh [debug|fast|release] [bundle 列表]"
    exit 1
    ;;
esac

echo
echo "==> 产物"
BUNDLE_DIR="${CARGO_TARGET_DIR:-$ROOT/src-tauri/target}/release/bundle"
if [ -d "$BUNDLE_DIR" ]; then
  find "$BUNDLE_DIR" -maxdepth 2 \( -name "*.app" -o -name "*.dmg" -o -name "*.exe" -o -name "*.msi" -o -name "*.AppImage" -o -name "*.deb" \) \
    -exec sh -c 'du -sh "$1" | sed "s|^|  |"' _ {} \;
else
  echo "  （没有找到 bundle 目录：$BUNDLE_DIR）"
fi
