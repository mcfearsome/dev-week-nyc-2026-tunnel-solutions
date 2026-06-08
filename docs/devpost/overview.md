# Project Overview

## Project Name

### tunnel.solutions

## Elevator Pitch

_Devpost caps this field at 200 characters — the pitch below is 181._

A pluggable host for ephemeral, capability-scoped tunnels to local MCP servers. Share only the tools you allow, on a link that expires. First plugin: send big files over BitTorrent.

**Live at https://tunnel.locker** · Rust · official MCP SDK (`rmcp`) · no inbound ports.

## Links

- Live relay + landing page: https://tunnel.locker
- Source: https://github.com/mcfearsome/tunnel-solutions
- Design specs + phased implementation plans: `docs/superpowers/`

## Note on architecture state

The project is mid-migration to the host + plugin architecture described here (designed spec-first; renames landed, the plugin extraction and `rmcp` port in flight). The capability-tunnel core and the BitTorrent file-sharing capability both work today and are deployed live at the URL above.
