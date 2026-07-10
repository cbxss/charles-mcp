# charles-mcp

An [MCP](https://modelcontextprotocol.io) server (Rust) that lets an AI agent drive [Charles Proxy 5](https://www.charlesproxy.com/) and inspect its HTTP(S) traffic — record, throttle, toggle tools, and list / search / read / **replay** requests with bodies decoded (gzip/brotli/base64, protobuf/gRPC, WebSocket) and JSON pretty-printed. Works on the live session or a saved `.har` / `.chlsj` / `.chls`. Traffic is ingested once into an embedded SQLite store (FTS5 search + content-addressed body dedup), so it stays fast on big sessions.

## Setup

1. In Charles: enable **Proxy → Web Interface Settings**.
2. Install the prebuilt binary (macOS arm64/intel, Linux x64):
   ```bash
   curl -fsSL https://raw.githubusercontent.com/cbxss/charles-mcp/main/install.sh | sh
   ```
   …or build from source: `cargo build --release` (bundles SQLite, needs a C compiler).
3. Register it with your MCP client:
   ```bash
   claude mcp add charles -- ~/.local/bin/charles-mcp
   ```

Defaults assume Charles on `127.0.0.1:8888`. Run **`charles_status`** first. No Charles? Point `read_session_file` at a saved `.har` / `.chlsj` / `.chls`.

## Tools

**Control:** `charles_status` · `start_recording` · `stop_recording` · `set_throttling` · `get_throttling` · `set_tool` · `get_tool_status` · `write_interception_rules` · `clear_session` · `quit_charles` · `reset_store`

**Inspect:** `list_requests` · `get_request` · `search_traffic` · `get_session_stats` · `replay_request` · `export_session` · `read_session_file` · `get_websocket_messages`

- **`list_requests`** sorts by priority and tags each row with a resource class. Filter by host/method/status/mime/resource_class/min_priority/path_regex, or `only_new: true` for a live tail.
- **`search_traffic`** is FTS5 over URLs, headers, and decoded bodies; `regex: true` for a body regex scan instead.
- **`replay_request`** re-issues a captured request to its origin (host fixed to the capture). Needs `confirm: true`, plus `allow_mutating: true` for POST/PUT/PATCH/DELETE.
- **`write_interception_rules`** writes Charles-native settings XML for Map Local, Map Remote, and Rewrite rules. Use `enable_tools: true` to turn on the matching Charles engines. Use `save_to_charles_config: true` with `confirm: true` to back up and merge the rules into the persisted Charles config.
- Destructive tools — `clear_session`, `quit_charles`, `reset_store`, and `set_tool(breakpoints, enable)` — need `confirm: true`.

## Interception rules

Charles' Web Interface can toggle Map Local, Map Remote, and Rewrite, but it does not expose live rule CRUD. `write_interception_rules` handles that by generating Charles settings XML and, optionally, saving it into Charles' config file.

Typical flow:

1. Call `write_interception_rules` with `map_local`, `map_remote`, and/or `rewrite_sets`.
2. Set `enable_tools: true` if Charles should enable the corresponding engines immediately.
3. Set `save_to_charles_config: true` and `confirm: true` to merge the rules into Charles' config after making a timestamped backup.

Saved config rules require Charles to restart or reload before live traffic uses the new definitions. If you only need a file to import manually, leave `save_to_charles_config` false and import the generated XML with **Tools → Import/Export Settings**.

## Configuration

Every flag has a `CHARLES_*` env fallback; run `--help` for the full list. Common ones:

| Flag | Default | |
| --- | --- | --- |
| `--proxy-host` / `--proxy-port` | `127.0.0.1` / `8888` | Charles proxy |
| `--web-user` / `--web-pass` | _(none)_ | Web Interface basic auth |
| `--db-path` | _(none)_ | persist the store (default: in-memory) |
| `--charles-config-path` | platform default | config file used by `write_interception_rules(save_to_charles_config)` |

For named protobuf/gRPC decoding, pass a `.proto` file per call: `get_request(index, proto_file="/abs/path/api.proto", proto_type="pkg.Msg")`. `proto_type` may be a short or fully-qualified name and can be omitted when the file defines a single message; `proto_root` sets the import root for `.proto` files that `import` others (defaults to the file's directory). The same args work on `get_websocket_messages` for binary frames.

## How it connects

The Web Interface isn't a normal port — it's reached *through* the proxy at the magic host `control.charles`:

```
charles-mcp ──HTTP──▶ Charles proxy (127.0.0.1:8888) ──internal──▶ control.charles
```

## Notes

- **HTTPS needs SSL Proxying** enabled per host in Charles, or requests stay opaque CONNECT tunnels (flagged as such, not faked as empty).
- **`set_tool` is master switches only** — use `write_interception_rules` for Map Local / Map Remote / Rewrite definitions. `breakpoints` still hangs matching traffic waiting for the Charles GUI (hence the `confirm`).
- **Live reads pull the whole session** each `--cache-ttl-ms` refresh (Charles has no delta API); the server's own `control.charles` traffic is dropped to keep the store lean.
- The `.chlsj` schema, the decoders, and the live endpoints are grounded in real Charles 5 captures.
