#!/usr/bin/env bash
# DeskTodo 构建环境（source 后使用）
#
# 本机没有全局 Rust 工具链（沙箱不允许写入 ~/.cargo），工具链安装在工作区内：
#   .rust-toolchain/  —— 由 scripts/bootstrap-rust.sh 安装
#
# 用法：  source scripts/env.sh
#         cargo build          # 在 desk-todo/src-tauri 下执行

set -u

_DESK_TODO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")/.." && pwd)"
_WS_ROOT="$(cd "$_DESK_TODO_ROOT/.." && pwd)"

# Rust 工具链（优先工作区内的，其次系统）
if [ -x "$_WS_ROOT/.rust-toolchain/bin/cargo" ]; then
  export RUSTUP_HOME="$_WS_ROOT/.rust-toolchain"
  export CARGO_HOME="$_WS_ROOT/.rust-toolchain"
  export PATH="$_WS_ROOT/.rust-toolchain/bin:$PATH"
elif [ -x "$HOME/.cargo/bin/cargo" ]; then
  export PATH="$HOME/.cargo/bin:$PATH"
fi

# 构建产物放进工作区，避免写沙箱外目录
export CARGO_TARGET_DIR="$_WS_ROOT/.cargo-target"
# 默认开启增量编译，加快迭代
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-1}"

# Node/pnpm（DSH 运行时自带）
_NODE_BIN="/Users/huerli/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/node/bin"
if [ -d "$_NODE_BIN" ]; then
  export PATH="$_NODE_BIN:$PATH"
fi

export DESK_TODO_ROOT="$_DESK_TODO_ROOT"
export WORKSPACE_ROOT="$_WS_ROOT"

echo "[env] rustc   : $(command -v rustc || echo '未找到') $(rustc --version 2>/dev/null)"
echo "[env] node    : $(command -v node || echo '未找到') $(node -v 2>/dev/null)"
echo "[env] target  : $CARGO_TARGET_DIR"
