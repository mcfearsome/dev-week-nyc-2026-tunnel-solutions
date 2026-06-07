#!/usr/bin/env bash
# Boot the relay, then open a 2-minute read-only tunnel to the demo MCP server.
set -euo pipefail

TTL="${TTL:-2m}"
SCOPE="${SCOPE:-read}"

cargo build -p relay -p mcp-demo -p tunnel-locker

cargo run -q -p relay &
RELAY_PID=$!
trap 'kill "$RELAY_PID" 2>/dev/null || true' EXIT
# Wait until the relay accepts connections — more robust than a fixed sleep for a live take.
for _ in $(seq 1 50); do
  curl -sf http://127.0.0.1:8787/healthz >/dev/null 2>&1 && break
  sleep 0.1
done

echo "relay up (pid $RELAY_PID). opening tunnel: --ttl $TTL --scope $SCOPE"
cargo run -q -p tunnel-locker -- open ./target/debug/mcp-demo --ttl "$TTL" --scope "$SCOPE"
