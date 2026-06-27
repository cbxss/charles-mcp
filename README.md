# charles-mcp

An [MCP](https://modelcontextprotocol.io) server (Rust) that lets an AI agent **control [Charles Proxy 5](https://www.charlesproxy.com/) and inspect the HTTP(S) traffic it captures** — start/stop recording, throttle, toggle tools, and list/search/read/**replay** requests with bodies decoded (gzip/brotli/base64, protobuf/gRPC, WebSocket) and JSON pretty-printed. Works on the **live** Charles session or on saved `.har` / `.chlsj` / `.chls` files.

Captured traffic is ingested once into an embedded **SQLite store** (FTS5 full-text search + content-addressed body dedup), so browsing, searching, and stats stay fast on large sessions and requests can be replayed against their origin.

## Setup

1. **In Charles:** turn on **Proxy → Web Interface Settings** (this is how the server talks to Charles).
2. **Build:**
   ```bash
   cargo build --release
   ```
   (Bundles SQLite from source, so a C compiler must be available at build time.)
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

That's it — defaults assume Charles on `127.0.0.1:8888`. Ask the agent to run **`charles_status`** first to confirm the connection.

> **No Charles running?** You can still inspect a saved session: point `read_session_file` at a `.har` or `.chlsj` (or `.chls`, if Charles is installed to convert it).

## Tools

**Control:** `charles_status` · `start_recording` · `stop_recording` · `set_throttling` · `set_tool` · `get_tool_status` · `clear_session`\* · `quit_charles`\* · `reset_store`\*  (\* destructive — need `confirm: true`)

**Inspect** (live session, or pass `file_path` for a saved file): `list_requests` · `get_request` · `search_traffic` · `get_session_stats` · `replay_request` · `export_session` · `read_session_file` · `get_websocket_messages`

Bodies are decoded automatically: gzip/brotli/base64, JSON pretty-printed, **protobuf/gRPC → a field tree** (schemaless, or named with `--proto-dir`), and **WebSocket frames** (RFC 6455, incl. protobuf-over-WS) via `get_websocket_messages`.

- **`list_requests`** sorts by a priority score and tags each row with a *resource class* (api_candidate / document / script / static_asset / …), so API and error traffic surfaces above asset noise. Filter with `resource_class`, `min_priority`, `host`, `method`, `status`, `path_regex`, or `mime`.
- **`search_traffic`** is FTS5 full-text over URLs, headers, and **decoded** bodies (including protobuf field trees); pass `regex: true` for a precise decoded-body regex scan instead.
- **`replay_request`** re-issues a captured request to its origin and shows the decoded response + a baseline diff. Safe by default: `confirm: true` to send, plus `allow_mutating: true` for POST/PUT/PATCH/DELETE; the target host is fixed to the capture (no host override), the proxy is off unless `use_proxy: true`. Override query params, headers, a JSON body, or the raw body.

## Configuration (optional)

Every flag has an env-var fallback; precedence is **CLI flag > env var > default**.

| Flag / env var | Default | Purpose |
| --- | --- | --- |
| `--proxy-host` / `CHARLES_PROXY_HOST` | `127.0.0.1` | Charles proxy host (point elsewhere if Charles runs on another machine) |
| `--proxy-port` / `CHARLES_PROXY_PORT` | `8888` | Charles proxy port |
| `--web-user` / `--web-pass` | _(none)_ | Web Interface basic auth, if you set one |
| `--charles-bin` / `CHARLES_BIN` | `/Applications/Charles.app/Contents/MacOS/Charles` | used to convert `.chls` files |
| `--body-max-bytes` / `CHARLES_BODY_MAX_BYTES` | `8192` | max decoded body bytes returned by `get_request` |
| `--proto-dir` / `CHARLES_PROTO_DIR` | _(none)_ | dir of `.proto` files for named protobuf/gRPC decoding (build with default feature `proto`) |
| `--export-timeout-ms` / `CHARLES_EXPORT_TIMEOUT_MS` | `60000` | timeout for reading the whole live session (separate from `--timeout-ms`; large captures take longer) |
| `--db-path` / `CHARLES_DB_PATH` | _(none)_ | persist the SQLite store to this path; default is an ephemeral in-memory store |
| `--store-max-captures` / `CHARLES_STORE_MAX_CAPTURES` | `10` | max stored *file* captures kept (LRU-evicted; live is always retained) |
| `--fts-body-max-bytes` / `CHARLES_FTS_BODY_MAX_BYTES` | `65536` | cap on decoded body text indexed per message for full-text search |

Set `CHARLES_LOG=debug` for verbose logs (written to stderr, never the stdio transport).

## How it connects

Charles's Web Interface isn't a normal port — it's reached *through* the Charles proxy at the magic host `http://control.charles/`. The server routes its control requests through the proxy, and Charles resolves that host internally:

```
charles-mcp ──HTTP──▶ Charles proxy (127.0.0.1:8888) ──internal──▶ control.charles
```

## Limitations (read before trusting it on a live session)

- **HTTPS needs SSL Proxying.** Charles only decrypts a host's HTTPS if you've enabled **Proxy → SSL Proxying** for it. Undecrypted requests are CONNECT tunnels — this server flags them explicitly (`⚠ HTTPS tunnel — not decrypted`) rather than pretending the body is missing.
- **`set_tool` toggles master switches only.** `map-*`, `rewrite`, `*-list`, and `dns-spoofing` do nothing without **rules** (GUI-only; not managed here). **`breakpoints` will pause/hang matching traffic** waiting for manual action in Charles — this server can't respond to breakpoints. Both cases are called out in the tool output.
- **Live inspection reads the session once and ingests it into the store** (refreshed every `--cache-ttl-ms`, default 5s, which also keeps indices stable in a burst). The live read uses Charles's `/session/export-json`; a session containing WebSocket frames falls back to a native download + `charles convert` (the JSON export omits WS frames). Bounded by `--export-timeout-ms`.
- **The SQLite store is ephemeral by default** (in-memory; gone on exit). Set `--db-path` to persist captures across restarts. `reset_store` (with `confirm: true`) drops everything.
- **`charles convert`** runs the Charles binary; it can collide with a running instance. Bounded by `--convert-timeout-ms`.
- WebSockets, gRPC, and protobuf **are** decoded now (frames / field trees). Schemaless protobuf needs no `.proto`; point `--proto-dir` at `.proto` files and pass `proto_type` to `get_request` for named fields.

## Validation & credit

The `.chlsj` schema, the decoders (protobuf/gRPC/WebSocket), and the live `/session/export-json` path are grounded in **real Charles 5 captures** (a schema mismatch fails loudly rather than returning blank rows). The control verbs (recording/throttling/tools), basic auth, and proxy routing follow the Charles docs; the `.har` 1.2 path is standard.

The SQLite store, the resource classifier, and replay were informed by the prior-art Python implementation [**heizaheiza/Charles-mcp**](https://github.com/heizaheiza/Charles-mcp) — with an improved schema (FTS5 full-text search + content-addressed body dedup, neither of which it has) and a decoder it lacks (protobuf/gRPC/WebSocket/brotli).
