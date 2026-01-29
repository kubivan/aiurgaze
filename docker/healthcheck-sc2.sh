#!/usr/bin/env bash
# Simple TCP healthcheck for StarCraft II.
# Returns 0 if the configured port(s) are accepting TCP connections on localhost.
# In multiplayer mode (SC2_MULTIPLAYER=1), checks both PORT and PORT+1.
set -euo pipefail

HOST=127.0.0.1
PORT=${SC2_PORT:-5555}

check_port() {
  bash -c "cat < /dev/tcp/${HOST}/$1 > /dev/null 2>&1"
}

if ! check_port "$PORT"; then
  exit 1
fi

if [ "${SC2_MULTIPLAYER:-0}" = "1" ]; then
  PORT2=$((PORT + 1))
  if ! check_port "$PORT2"; then
    exit 1
  fi
fi

exit 0
