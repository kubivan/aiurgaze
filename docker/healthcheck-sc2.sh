#!/usr/bin/env bash
# Simple TCP healthcheck for StarCraft II.
# Returns 0 if the configured port is accepting TCP connections on localhost.
set -euo pipefail

HOST=127.0.0.1
PORT=${SC2_PORT:-5555}

# Use bash /dev/tcp to test connectivity. This will succeed if the socket is open.
if bash -c "cat < /dev/tcp/${HOST}/${PORT} > /dev/null 2>&1"; then
  exit 0
else
  exit 1
fi
