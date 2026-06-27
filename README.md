# charles-mcp

An [MCP](https://modelcontextprotocol.io) server (Rust) that lets an AI agent **drive [Charles Proxy 5](https://www.charlesproxy.com/) and inspect the HTTP(S) traffic it captures**: start/stop recording, throttle, toggle tools, and list / search / read / **replay** requests with bodies decoded (gzip/brotli/base64, protobuf/gRPC, WebSocket) and JSON pretty-printed. Works on the **live** Charles session or a saved `.har` / `.chlsj` / `.chls` file.

Captured traffic is ingested once into an embedded **SQLite store** (FTS5 full-text search + content-addressed body dedup), so browsing, searching, and stats stay fast on large sessions.

## Setup

1. **In Charles:** turn on **Proxy → Web Interface Settings** — this is how the server talks to Charles.
2. **Build** (bundles SQLite from source, so a C compiler must be present):
   ```bash
   cargo build --release
   ```
3. **Add to Claude Code:**
   ```bash
   claude mcp add charles -- "$(pwd)/target/release/charles-mcp"
   ```
   …or in `.mcp.json` / `claude_desktop_config.json`:
   ```json
   {
     "mcpServers": {
       "charles": { "command": "/abs/path/to/target/release/charles-mcp" }
     }
   }
   ```

Defaults assume Charles on `127.0.0.1:8888`. Ask the agent to run **`charles_status`** first to confirm the connection.

> **No Charles running?** Point `read_session_file` at a saved `.har` or `.chlsj` (or `.chls`, if Charles is installed to convert it).

## Tools

**Control:** `charles_status` · `start_recording` · `stop_recording` · `set_throttling` · `set_tool` · `get_tool_status` · `clear_session`\* · `quit_charles`\* · `reset_store`\*  (\* destructive — need `confirm: true`)

**Inspect** (live session, or pass `file_path` for a saved file): `list_requests` · `get_request` · `search_traffic` · `get_session_stats` · `replay_request` · `export_session` · `read_session_file` · `get_websocket_messages`

Bodies decode automatically: gzip/brotli/base64, JSON pretty-printed, **protobuf/gRPC → a field tree** (schemaless, or named with `--proto-dir`), and **WebSocket frames** (RFC 6455, incl. protobuf-over-WS) via `get_websocket_messages`.

- **`list_requests`** sorts by a priority score and tags each row with a *resource class* (api_candidate / document / script / static_asset / …), so API and error traffic surfaces above asset noise. Filter with `resource_class`, `min_priority`, `host`, `method`, `status`, `path_regex`, or `mime`.
- **`search_traffic`** is FTS5 full-text over URLs, headers, and **decoded** bodies (including protobuf field trees); pass `regex: true` for a decoded-body regex scan instead.
- **`replay_request`** re-issues a captured request to its origin and shows the decoded response + a baseline diff. Safe by default: `confirm: true` to send, plus `allow_mutating: true` for POST/PUT/PATCH/DELETE. The target host is fixed to the capture (no host override) and the proxy is off unless `use_proxy: true`. Override query params, headers, a JSON body, or the raw body.

## Configuration

Optional. Every flag has an env-var fallback; precedence is **CLI flag > env var > default**.

| Flag / env var | Default | Purpose |
| --- | --- | --- |
| `--proxy-host` / `CHARLES_PROXY_HOST` | `127.0.0.1` | Charles proxy host (point elsewhere if Charles runs on another machine) |
| `--proxy-port` / `CHARLES_PROXY_PORT` | `8888` | Charles proxy port |
| `--control-host` / `CHARLES_CONTROL_HOST` | `control.charles` | Magic host the Web Interface answers on (reached through the proxy) |
| `--web-user` / `CHARLES_WEB_USER` | _(none)_ | Web Interface basic-auth user, if you set one |
| `--web-pass` / `CHARLES_WEB_PASS` | _(none)_ | Web Interface basic-auth password, if you set one |
| `--charles-bin` / `CHARLES_BIN` | `/Applications/Charles.app/Contents/MacOS/Charles` | Charles binary, used to convert `.chls` files |
| `--timeout-ms` / `CHARLES_TIMEOUT_MS` | `15000` | Per-request timeout for control/inspect calls |
| `--export-timeout-ms` / `CHARLES_EXPORT_TIMEOUT_MS` | `60000` | Timeout for reading the whole live session (large captures take longer than a control call) |
| `--convert-timeout-ms` / `CHARLES_CONVERT_TIMEOUT_MS` | `60000` | Timeout for the `charles convert` subprocess |
| `--cache-ttl-ms` / `CHARLES_CACHE_TTL_MS` | `5000` | How long a live snapshot is reused (also keeps request indices stable in a burst); `0` disables |
| `--default-export-format` / `CHARLES_EXPORT_FORMAT` | `chlsj` | Preferred format when fetching the live session |
| `--body-max-bytes` / `CHARLES_BODY_MAX_BYTES` | `8192` | Max decoded body bytes returned by `get_request` |
| `--proto-dir` / `CHARLES_PROTO_DIR` | _(none)_ | Dir of `.proto` files for named protobuf/gRPC decoding (`proto` feature, on by default) |
| `--db-path` / `CHARLES_DB_PATH` | _(none)_ | Persist the SQLite store here; default is ephemeral (in-memory) |
| `--store-max-captures` / `CHARLES_STORE_MAX_CAPTURES` | `10` | Max stored *file* captures kept (LRU-evicted; live is always retained) |
| `--fts-body-max-bytes` / `CHARLES_FTS_BODY_MAX_BYTES` | `65536` | Cap on decoded body text indexed per message for full-text search |

Set `CHARLES_LOG=debug` for verbose logs (written to stderr, never the stdio transport).

## How it connects

The Charles Web Interface isn't a normal port — it's reached *through* the Charles proxy at the magic host `http://control.charles/`. The server routes its control requests through the proxy, and Charles resolves that host internally:

```
charles-mcp ──HTTP──▶ Charles proxy (127.0.0.1:8888) ──internal──▶ control.charles
```

## Limitations

- **HTTPS needs SSL Proxying.** Charles only decrypts a host's HTTPS if you've enabled **Proxy → SSL Proxying** for it. Undecrypted requests are CONNECT tunnels; the server flags them (`⚠ HTTPS tunnel — not decrypted`) rather than pretending the body is missing.
- **`set_tool` toggles master switches only.** `map-*`, `rewrite`, `*-list`, and `dns-spoofing` do nothing without **rules** (GUI-only; not managed here). **`breakpoints` pauses/hangs matching traffic** waiting for manual action in Charles, which this server can't supply. Both cases are called out in the tool output.
- **Live inspection reads the session once and ingests it,** refreshed every `--cache-ttl-ms` (also keeps indices stable in a burst). The read uses Charles's `/session/export-json`; a session with WebSocket frames falls back to a native download + `charles convert`, since the JSON export omits WS frames. Bounded by `--export-timeout-ms`.
- **`charles convert`** runs the Charles binary and can collide with a running instance. Bounded by `--convert-timeout-ms`.
- **The SQLite store is ephemeral by default** (in-memory; gone on exit). Set `--db-path` to persist captures; `reset_store` (with `confirm: true`) drops everything.

## Validation & credit

The `.chlsj` schema, the decoders (protobuf/gRPC/WebSocket), and the live `/session/export-json` path are grounded in **real Charles 5 captures** (a schema mismatch fails loudly rather than returning blank rows). The control verbs (recording/throttling/tools), basic auth, and proxy routing follow the Charles docs; the `.har` 1.2 path is standard.

The SQLite store, the resource classifier, and replay were informed by the prior-art Python implementation [**heizaheiza/Charles-mcp**](https://github.com/heizaheiza/Charles-mcp) — with an improved schema (FTS5 full-text search + content-addressed body dedup, neither of which it has) and a decoder it lacks (protobuf/gRPC/WebSocket/brotli).
