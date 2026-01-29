#!/usr/bin/env bash
# Helper script to launch StarCraft II headless.
# Supports multiplayer: set SC2_MULTIPLAYER=1 to start two SC2 instances
# on SC2_PORT (default 5555) and SC2_PORT+1 (default 5556).
#
# You can pass additional SC2 flags after a -- delimiter, e.g.:
#   ./run-sc2.sh -- -port 8167 -listen 0.0.0.0
set -euo pipefail
BASE_DIR="/StarCraftII"
BIN=$(ls -d ${BASE_DIR}/Versions/Base* | sort | tail -n1)/SC2_x64
if [ ! -x "$BIN" ]; then
  echo "Could not find SC2 binary at $BIN" >&2
  exit 1
fi

PORT1=${SC2_PORT:-5555}
PORT2=$((PORT1 + 1))

# Common flags (without port, which differs per instance)
COMMON_FLAGS=(
  -listen 0.0.0.0
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

if [ "${SC2_MULTIPLAYER:-0}" = "1" ]; then
  echo "[run-sc2] Multiplayer mode: starting SC2 on ports $PORT1 and $PORT2"
  # Launch first instance in the background
  "$BIN" "${COMMON_FLAGS[@]}" -port "$PORT1" "${EXTRA_FLAGS[@]}" &
  PID1=$!
  # Launch second instance in the foreground
  "$BIN" "${COMMON_FLAGS[@]}" -port "$PORT2" "${EXTRA_FLAGS[@]}" &
  PID2=$!
  # Wait for either to exit, then kill both
  wait -n "$PID1" "$PID2" 2>/dev/null || true
  kill "$PID1" "$PID2" 2>/dev/null || true
  wait
else
  exec "$BIN" "${COMMON_FLAGS[@]}" -port "$PORT1" "${EXTRA_FLAGS[@]}"
fi