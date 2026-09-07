#!/usr/bin/env sh
set -eu

PROJECT_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
PYTHON_EXE="$PROJECT_ROOT/python/.venv/bin/python"
if [ ! -x "$PYTHON_EXE" ]; then
  PYTHON_EXE=python3
fi

cleanup() {
  kill "$FLASK_PID" "$OTA_PID" 2>/dev/null || true
}
trap cleanup INT TERM EXIT

echo "CSV Profiler: http://localhost:5000"
(cd "$PROJECT_ROOT" && "$PYTHON_EXE" python/profiler_server.py) &
FLASK_PID=$!

echo "Rust OTA:     http://localhost:7000"
(cd "$PROJECT_ROOT/ota-server" && cargo run) &
OTA_PID=$!

echo "Set OTA_PUBLIC_BASE_URL in .env to this PC's LAN address before deployment."
wait "$FLASK_PID" "$OTA_PID"

