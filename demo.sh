#!/usr/bin/env bash
# Boot the relay, then open a 2-minute read-only tunnel to the demo MCP server.
set -euo pipefail

TTL="${TTL:-2m}"
SCOPE="${SCOPE:-read}"

cargo build -p relay -p mcp-demo -p agent

cargo run -q -p relay &
RELAY_PID=$!
trap 'kill "$RELAY_PID" 2>/dev/null || true' EXIT
sleep 1

echo "relay up (pid $RELAY_PID). opening tunnel: --ttl $TTL --scope $SCOPE"
cargo run -q -p agent -- open ./target/debug/mcp-demo --ttl "$TTL" --scope "$SCOPE"
