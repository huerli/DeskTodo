#!/usr/bin/env bash
# 生成图标（不依赖 tauri CLI，纯 Python + Pillow）
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

PY="${PYTHON:-python3}"
if ! "$PY" -c "import PIL" 2>/dev/null; then
  for cand in /Users/huerli/.dsh/dsh-runtimes/dsh-primary-runtime/dependencies/python/bin/python3; do
    if [ -x "$cand" ] && "$cand" -c "import PIL" 2>/dev/null; then PY="$cand"; break; fi
  done
fi

echo "==> 使用 $PY"
"$PY" scripts/make_icon.py
"$PY" scripts/gen_icons.py
echo "==> 图标已生成到 src-tauri/icons/"
