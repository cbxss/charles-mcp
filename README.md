# charles-mcp

An [MCP](https://modelcontextprotocol.io) server (Rust) that lets an AI agent **control [Charles Proxy 5](https://www.charlesproxy.com/) and inspect the HTTP(S) traffic it captures** — start/stop recording, throttle, toggle tools, and list/search/read requests with bodies decoded (gzip/brotli/base64) and JSON pretty-printed. Works on the **live** Charles session or on saved `.har` / `.chlsj` / `.chls` files.

## Setup

1. **In Charles:** turn on **Proxy → Web Interface Settings** (this is how the server talks to Charles).
2. **Build:**
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

That's it — defaults assume Charles on `127.0.0.1:8888`. Ask the agent to run **`charles_status`** first to confirm the connection.

> **No Charles running?** You can still inspect a saved session: point `read_session_file` at a `.har` or `.chlsj` (or `.chls`, if Charles is installed to convert it).

## Tools

**Control:** `charles_status` · `start_recording` · `stop_recording` · `set_throttling` · `set_tool` · `get_tool_status` · `clear_session`\* · `quit_charles`\*  (\* destructive — need `confirm: true`)

**Inspect** (live session, or pass `file_path` for a saved file): `list_requests` · `get_request` · `search_traffic` · `get_session_stats` · `export_session` · `read_session_file` · `get_websocket_messages`

Bodies are decoded automatically: gzip/brotli/base64, JSON pretty-printed, **protobuf/gRPC → a field tree** (schemaless, or named with `--proto-dir`), and **WebSocket frames** (RFC 6455, incl. protobuf-over-WS) via `get_websocket_messages`.

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

Set `CHARLES_LOG=debug` for verbose logs (written to stderr, never the stdio transport).

## How it connects

Charles's Web Interface isn't a normal port — it's reached *through* the Charles proxy at the magic host `http://control.charles/`. The server routes its control requests through the proxy, and Charles resolves that host internally:

```
charles-mcp ──HTTP──▶ Charles proxy (127.0.0.1:8888) ──internal──▶ control.charles
```

## Limitations (read before trusting it on a live session)

- **HTTPS needs SSL Proxying.** Charles only decrypts a host's HTTPS if you've enabled **Proxy → SSL Proxying** for it. Undecrypted requests are CONNECT tunnels — this server flags them explicitly (`⚠ HTTPS tunnel — not decrypted`) rather than pretending the body is missing.
- **`set_tool` toggles master switches only.** `map-*`, `rewrite`, `*-list`, and `dns-spoofing` do nothing without **rules** (GUI-only; not managed here). **`breakpoints` will pause/hang matching traffic** waiting for manual action in Charles — this server can't respond to breakpoints. Both cases are called out in the tool output.
- **Live inspection re-exports the session** (cached for `--cache-ttl-ms`, default 5s, which also keeps indices stable in a burst). Large sessions can be slow or hit `--timeout-ms`.
- **`charles convert`** runs the Charles binary; it can collide with a running instance. Bounded by `--convert-timeout-ms`.
- WebSockets, gRPC, and protobuf **are** decoded now (frames / field trees). Schemaless protobuf needs no `.proto`; point `--proto-dir` at `.proto` files and pass `proto_type` to `get_request` for named fields.

## Note: provisional schema ⚠️

Built and tested **without a live Charles 5**, so the exact `.chlsj` field names and the session export/clear/quit endpoint paths are best-effort until validated against a real install (a schema mismatch now fails loudly instead of returning blank rows). The control verbs (recording/throttling/tools), basic auth, proxy routing, and the `.har` 1.2 path are grounded in the docs. 53 fixture-driven tests pass (`cargo test`).

<details>
<summary>Validate against a real Charles 5 (and lock in real fixtures)</summary>

1. Enable **Proxy → Web Interface Settings** (note any username/password).
2. Send traffic through the proxy (`127.0.0.1:8888`) so the session has varied HTTP/HTTPS requests.
3. Run the server under Claude or the [MCP Inspector](https://github.com/modelcontextprotocol/inspector).
4. `charles_status` → expect `reachable: true`.
5. `start_recording` / `stop_recording` → watch the Charles UI toggle.
6. `set_throttling` (each preset), `set_tool` + `get_tool_status` (every tool) → confirm round-trips.
7. `list_requests` / `get_request` / `search_traffic` / `get_session_stats` → confirm bodies decode (gzip/brotli/base64, JSON, binaries).
8. `export_session` to `chlsj` and `har`, then `read_session_file` them back.
9. `clear_session`, then `quit_charles` (each `confirm: true`).
10. Drop a real control-page HTML + real `.chlsj`/`.har` into `tests/fixtures/`, then re-run `cargo test` to lock the schema and endpoint discovery to ground truth.

</details>
