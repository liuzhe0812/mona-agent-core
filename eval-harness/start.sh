#!/usr/bin/env sh
set -eu
cd "$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
PYTHON="${PYTHON:-python3}"
if ! command -v "$PYTHON" >/dev/null 2>&1; then
  printf '%s\n' 'Python 3.11+ is required; install it before starting.' >&2
  exit 2
fi
"$PYTHON" -c 'import sys,sqlite3; sys.exit(0 if sys.version_info >= (3,11) else 1)' || {
  printf '%s\n' 'Python 3.11+ with sqlite3 is required.' >&2
  exit 2
}
export PYTHONUTF8=1 PYTHONDONTWRITEBYTECODE=1
exec "$PYTHON" -m harness serve --open "$@"
