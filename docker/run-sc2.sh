#!/usr/bin/env bash
# Helper script to launch StarCraft II headless manually.
# You can pass additional SC2 flags after a -- delimiter, e.g.:
#   ./run-sc2.sh -- -port 8167 -listen 0.0.0.0
set -euo pipefail
BASE_DIR="/StarCraftII"
BIN=$(ls -d ${BASE_DIR}/Versions/Base* | sort | tail -n1)/SC2_x64
if [ ! -x "$BIN" ]; then
  echo "Could not find SC2 binary at $BIN" >&2
  exit 1
fi
DEFAULT_FLAGS=(
  -listen 0.0.0.0
  -port 5555
  -displayMode 0
  -dataDir ${BASE_DIR}
)
# Collect user extras after --
EXTRA_FLAGS=()
PASSTHRU=false
for arg in "$@"; do
  if $PASSTHRU; then
    EXTRA_FLAGS+=("$arg")
  elif [ "$arg" == "--" ]; then
    PASSTHRU=true
  fi
done
exec "$BIN" "${DEFAULT_FLAGS[@]}" "${EXTRA_FLAGS[@]}"