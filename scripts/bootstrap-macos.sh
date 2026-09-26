#!/usr/bin/env bash
# 一次性安装本机所需工具链（macOS）
#
# 说明：本机沙箱不允许写入 ~/.cargo / ~/.rustup，因此 Rust 工具链安装到
#       工作区内的 .rust-toolchain/ 目录，由 scripts/env.sh 统一注入环境。
#
# 用法： bash scripts/bootstrap-macos.sh

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WS_ROOT="$(cd "$ROOT/.." && pwd)"
TCDIR="$WS_ROOT/.rust-toolchain"
DL="$TCDIR/dl"

BASE="https://static.rust-lang.org/dist"

echo "==> 目标工具链目录: $TCDIR"

if [ -x "$TCDIR/bin/cargo" ]; then
  echo "==> 已安装: $("$TCDIR/bin/cargo" --version)，跳过"
  exit 0
fi

command -v curl >/dev/null || { echo "缺少 curl"; exit 1; }
command -v tar  >/dev/null || { echo "缺少 tar"; exit 1; }

mkdir -p "$DL"

echo "==> 解析 stable 版本号"
curl -fsSL --max-time 60 -o "$DL/channel.toml" "$BASE/channel-rust-stable.toml"
VER="$(sed -n '/^\[pkg.rust\]/,/^\[/p' "$DL/channel.toml" \
  | grep -m1 '^version' | sed -E 's/.*"([0-9]+\.[0-9]+\.[0-9]+).*/\1/')"
[ -n "$VER" ] || { echo "无法解析版本号"; exit 1; }
echo "    stable = $VER"

TARBALL="$DL/rust.tar.gz"
if [ ! -f "$TARBALL" ]; then
  echo "==> 下载 rust-$VER-aarch64-apple-darwin.tar.gz"
  curl -fL --retry 3 --max-time 1800 \
    -o "$TARBALL" "$BASE/rust-$VER-aarch64-apple-darwin.tar.gz"
fi

echo "==> 解压并安装"
rm -rf "$DL/rust-$VER-aarch64-apple-darwin"
tar -xzf "$TARBALL" -C "$DL"
"$DL/rust-$VER-aarch64-apple-darwin/install.sh" \
  --prefix="$TCDIR" --without=rust-docs --disable-ldconfig >/dev/null

echo "==> 完成"
"$TCDIR/bin/rustc" --version
"$TCDIR/bin/cargo" --version
echo
echo "接下来：  source scripts/env.sh"
