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

**Inspect** (live session, or pass `file_path` for a saved file): `list_requests` · `get_request` · `search_traffic` · `get_session_stats` · `export_session` · `read_session_file`

## Configuration (optional)

Every flag has an env-var fallback; precedence is **CLI flag > env var > default**.

| Flag / env var | Default | Purpose |
| --- | --- | --- |
| `--proxy-host` / `CHARLES_PROXY_HOST` | `127.0.0.1` | Charles proxy host (point elsewhere if Charles runs on another machine) |
| `--proxy-port` / `CHARLES_PROXY_PORT` | `8888` | Charles proxy port |
| `--web-user` / `--web-pass` | _(none)_ | Web Interface basic auth, if you set one |
| `--charles-bin` / `CHARLES_BIN` | `/Applications/Charles.app/Contents/MacOS/Charles` | used to convert `.chls` files |
| `--body-max-bytes` / `CHARLES_BODY_MAX_BYTES` | `8192` | max decoded body bytes returned by `get_request` |

Set `CHARLES_LOG=debug` for verbose logs (written to stderr, never the stdio transport).

## How it connects

Charles's Web Interface isn't a normal port — it's reached *through* the Charles proxy at the magic host `http://control.charles/`. The server routes its control requests through the proxy, and Charles resolves that host internally:

```
charles-mcp ──HTTP──▶ Charles proxy (127.0.0.1:8888) ──internal──▶ control.charles
```

## Note: provisional schema ⚠️

Built and tested **without a live Charles 5**, so the `.chlsj` field names and the Web-Interface endpoint paths are best-effort until validated against a real install. The `.har` path follows the public HAR 1.2 spec and is solid. 48 fixture-driven tests pass (`cargo test`).

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
