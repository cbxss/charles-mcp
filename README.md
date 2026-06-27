# charles-mcp

An [MCP](https://modelcontextprotocol.io) server (Rust) that lets an AI agent drive [Charles Proxy 5](https://www.charlesproxy.com/) and inspect its HTTP(S) traffic — record, throttle, toggle tools, and list / search / read / **replay** requests with bodies decoded (gzip/brotli/base64, protobuf/gRPC, WebSocket) and JSON pretty-printed. Works on the live session or a saved `.har` / `.chlsj` / `.chls`. Traffic is ingested once into an embedded SQLite store (FTS5 search + content-addressed body dedup), so it stays fast on big sessions.

## Setup

1. In Charles: enable **Proxy → Web Interface Settings**.
2. `cargo build --release` — bundles SQLite, so a C compiler must be present.
3. `claude mcp add charles -- "$(pwd)/target/release/charles-mcp"` (or point your MCP client at the binary).

Defaults assume Charles on `127.0.0.1:8888`. Run **`charles_status`** first. No Charles? Point `read_session_file` at a saved `.har` / `.chlsj` / `.chls`.

## Tools

**Control:** `charles_status` · `start_recording` · `stop_recording` · `set_throttling` · `get_throttling` · `set_tool` · `get_tool_status` · `clear_session` · `quit_charles` · `reset_store`

**Inspect:** `list_requests` · `get_request` · `search_traffic` · `get_session_stats` · `replay_request` · `export_session` · `read_session_file` · `get_websocket_messages`

- **`list_requests`** sorts by priority and tags each row with a resource class. Filter by host/method/status/mime/resource_class/min_priority/path_regex, or `only_new: true` for a live tail.
- **`search_traffic`** is FTS5 over URLs, headers, and decoded bodies; `regex: true` for a body regex scan instead.
- **`replay_request`** re-issues a captured request to its origin (host fixed to the capture). Needs `confirm: true`, plus `allow_mutating: true` for POST/PUT/PATCH/DELETE.
- Destructive tools — `clear_session`, `quit_charles`, `reset_store`, and `set_tool(breakpoints, enable)` — need `confirm: true`.

## Configuration

Every flag has a `CHARLES_*` env fallback; run `--help` for the full list. Common ones:

| Flag | Default | |
| --- | --- | --- |
| `--proxy-host` / `--proxy-port` | `127.0.0.1` / `8888` | Charles proxy |
| `--web-user` / `--web-pass` | _(none)_ | Web Interface basic auth |
| `--db-path` | _(none)_ | persist the store (default: in-memory) |
| `--proto-dir` | _(none)_ | `.proto` files for named protobuf/gRPC decoding |

## How it connects

The Web Interface isn't a normal port — it's reached *through* the proxy at the magic host `control.charles`:

```
charles-mcp ──HTTP──▶ Charles proxy (127.0.0.1:8888) ──internal──▶ control.charles
```

## Notes

- **HTTPS needs SSL Proxying** enabled per host in Charles, or requests stay opaque CONNECT tunnels (flagged as such, not faked as empty).
- **`set_tool` is master switches only** — the Web Interface exposes no rule or breakpoint management, so `map-*` / `rewrite` / `*-list` / `dns-spoofing` are no-ops without GUI rules, and `breakpoints` hangs matching traffic (hence the `confirm`).
- **Live reads pull the whole session** each `--cache-ttl-ms` refresh (Charles has no delta API); the server's own `control.charles` traffic is dropped to keep the store lean.
- The `.chlsj` schema, the decoders, and the live endpoints are grounded in real Charles 5 captures.
