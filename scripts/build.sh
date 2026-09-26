#!/usr/bin/env bash
# 本地构建：构建当前平台的可执行文件与安装包
#
#   bash scripts/build.sh             # release 构建（当前平台默认 bundle）
#   bash scripts/build.sh debug       # debug 构建，不打包
#   bash scripts/build.sh release app # 只出 .app
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

MODE="${1:-release}"
BUNDLES="${2:-}"

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
  debug)
    echo "==> debug 构建（不打包）"
    cargo build --manifest-path src-tauri/Cargo.toml
    echo "==> 可执行文件: ${CARGO_TARGET_DIR:-src-tauri/target}/debug/desk-todo"
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
    echo "用法: bash scripts/build.sh [debug|release] [bundle 列表]"
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
