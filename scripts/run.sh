#!/usr/bin/env bash
# 本机直接运行（默认 release，也可指定 debug）
#
#   bash scripts/run.sh                 # 前台运行 release 版，Ctrl+C 结束
#   bash scripts/run.sh debug           # 运行 debug 版
#   bash scripts/run.sh release --exit-after 8   # 冒烟：8 秒后自动退出
#
# 提示：前端资源是编译期内嵌的，改了 src/ 下的文件必须重新 build。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
# shellcheck disable=SC1091
source scripts/env.sh

PROFILE="${1:-release}"
shift || true

# 依次尝试：CARGO_TARGET_DIR、src-tauri/target
CANDIDATES=(
  "${CARGO_TARGET_DIR:-}/$PROFILE/desk-todo"
  "$ROOT/src-tauri/target/$PROFILE/desk-todo"
)
APP=""
for c in "${CANDIDATES[@]}"; do
  if [ -n "$c" ] && [ -x "$c" ]; then APP="$c"; break; fi
done

if [ -z "$APP" ]; then
  echo "未找到可执行文件（已尝试）："
  printf '  %s\n' "${CANDIDATES[@]}"
  echo "先执行： bash scripts/build.sh $PROFILE"
  exit 1
fi

echo "==> 启动 $APP $*"
exec "$APP" "$@"
