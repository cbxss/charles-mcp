# charles-mcp

An [MCP](https://modelcontextprotocol.io) (Model Context Protocol) stdio server, written in Rust, that lets an AI agent **control [Charles Proxy 5](https://www.charlesproxy.com/) and inspect the HTTP(S) traffic it captures**. It drives Charles's Web Interface to start/stop recording, toggle tools, and throttle bandwidth, and it parses Charles sessions (`.chls`, `.har`, `.chlsj`) into a normalized model so an agent can list, search, and read individual requests and responses — bodies decoded (gzip/deflate/brotli/base64), JSON pretty-printed, binaries summarized, and large payloads truncated to stay context-frugal.

## How it connects

Charles exposes a **Web Interface** (enable it in Charles under **Proxy → Web Interface Settings**). The Web Interface is **not** a normal HTTP port — it is reached *through* the Charles HTTP proxy at the magic host **`http://control.charles/`**.

So `charles-mcp` routes its control requests through the proxy (default `127.0.0.1:8888`); Charles resolves `control.charles` internally. If you protect the Web Interface with a username/password, supply them via `--web-user`/`--web-pass` (sent as HTTP basic auth on the `control.charles` request).

```
charles-mcp ──HTTP──▶ Charles proxy (127.0.0.1:8888) ──internal──▶ control.charles (Web Interface)
```

## Build & install

```bash
cargo build --release
# binary at: target/release/charles-mcp
```

It is a stdio MCP server: it reads JSON-RPC on stdin and writes on stdout (logs go to **stderr**). Run it from an MCP client rather than directly.

## Configuration

Every setting is a CLI flag with an environment-variable fallback. Precedence is **CLI flag > environment variable > default**.

| CLI flag | Environment variable | Default | Description |
| --- | --- | --- | --- |
| `--proxy-host` | `CHARLES_PROXY_HOST` | `127.0.0.1` | Host of the running Charles HTTP proxy. |
| `--proxy-port` | `CHARLES_PROXY_PORT` | `8888` | Port of the running Charles HTTP proxy. |
| `--control-host` | `CHARLES_CONTROL_HOST` | `control.charles` | Magic host the Web Interface answers on (through the proxy). |
| `--web-user` | `CHARLES_WEB_USER` | _(none)_ | Username for Web Interface basic auth, if configured. |
| `--web-pass` | `CHARLES_WEB_PASS` | _(none)_ | Password for Web Interface basic auth, if configured. |
| `--charles-bin` | `CHARLES_BIN` | `/Applications/Charles.app/Contents/MacOS/Charles` | Charles binary, used for `charles convert` of `.chls` files. |
| `--timeout-ms` | `CHARLES_TIMEOUT_MS` | `15000` | Per-request timeout (milliseconds). |
| `--body-max-bytes` | `CHARLES_BODY_MAX_BYTES` | `8192` | Default cap on decoded body bytes returned by `get_request`. |
| `--export-format` | `CHARLES_EXPORT_FORMAT` | `chlsj` | Preferred format when fetching/exporting the live session. |

Logging verbosity is controlled by **`CHARLES_LOG`** (a [tracing `EnvFilter`](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html), e.g. `CHARLES_LOG=debug`); logs are written to stderr so they never corrupt the stdio transport.

## Tools

Control (drive a running Charles via the Web Interface):

| Tool | Description |
| --- | --- |
| `charles_status` | Check connectivity to the Web Interface; report proxy, auth, and whether the Charles binary is available. Run this first. |
| `start_recording` | Start recording traffic in Charles. |
| `stop_recording` | Stop recording traffic in Charles. |
| `set_throttling` | Enable/disable bandwidth throttling, optionally with a preset (e.g. `3G`, `4G`, `56 kbps Modem`). |
| `set_tool` | Enable or disable a Charles tool (breakpoints, map-local, map-remote, rewrite, block-cookies, no-caching, dns-spoofing, auto-save, black-list, white-list, client-process). |
| `get_tool_status` | Report whether a given Charles tool is currently enabled or disabled. |
| `clear_session` | Clear the current Charles session (destructive; requires `confirm: true`). |
| `quit_charles` | Quit Charles (destructive; requires `confirm: true`). |

Inspect (parse and explore captured traffic):

| Tool | Description |
| --- | --- |
| `read_session_file` | Parse a `.chls`/`.har`/`.chlsj` file from disk and list its requests as a compact table. |
| `list_requests` | List captured requests as a compact, filterable table (host/method/status/path/mime/limit). |
| `get_request` | Full decoded detail for one request by index (headers, decoded+pretty body, timings). |
| `search_traffic` | Search captured traffic across URL, headers, and bodies (substring or regex). |
| `get_session_stats` | Aggregate stats: counts by host/status/mime, slowest requests, errors. |
| `export_session` | Export the current Charles session to a file in a chosen format (chlsj/har/chls/xml/csv). |

The inspect tools accept an optional `file_path` to read a session file; omit it to inspect the **live** Charles session.

## Wiring into a client

**Claude Code** (`claude mcp add`):

```bash
claude mcp add charles -- /abs/path/to/charles-mcp/target/release/charles-mcp
```

**`.mcp.json` / `claude_desktop_config.json`:**

```json
{
  "mcpServers": {
    "charles": {
      "command": "/abs/path/to/charles-mcp/target/release/charles-mcp",
      "env": {
        "CHARLES_PROXY_HOST": "127.0.0.1",
        "CHARLES_PROXY_PORT": "8888",
        "CHARLES_WEB_USER": "",
        "CHARLES_WEB_PASS": ""
      }
    }
  }
}
```

Drop the `CHARLES_WEB_USER`/`CHARLES_WEB_PASS` entries unless the Web Interface is password-protected. Point `CHARLES_PROXY_HOST` at another machine/device when Charles runs elsewhere.

## Offline workflow

You do **not** need a running Charles to inspect a saved session. `read_session_file` works fully offline:

- `.har` and `.chlsj` are parsed directly — no Charles required.
- `.chls` (Charles's native format) is converted first via `charles convert`, so it requires Charles to be installed (`--charles-bin` / `CHARLES_BIN`).

```jsonc
// tool call
{ "name": "read_session_file", "arguments": { "path": "/abs/path/to/session.har" } }
```

## Provisional fixtures ⚠️

This server was built and tested **without a live Charles 5 instance**. Two areas are therefore *provisional* until validated against a real Charles:

- **The `.chlsj` schema** — `tests/fixtures/sample.chlsj` reflects the Charles JSON session structure as reconstructed from documentation and a community importer. Real Charles output may differ in field names/encoding.
- **Web-Interface endpoint discovery** — `tests/fixtures/control_page.html` is a hand-authored stand-in. The live read path discovers the session-export/clear/quit endpoints by parsing the real control page, which can only be confirmed against a running Charles.

The `.har` path follows the public HAR 1.2 spec and is not provisional.

## Manual live-test checklist (with Charles 5 installed)

Run this once Charles 5 is available to validate the live paths and lock in real fixtures:

1. **Enable the Web Interface** in Charles: **Proxy → Web Interface Settings** (note any username/password).
2. **Generate traffic**: configure a browser or device to use the Charles proxy (`127.0.0.1:8888`) and load a few HTTP and HTTPS sites so the session has varied requests.
3. **Launch the server** under an MCP client (Claude, or the [MCP Inspector](https://github.com/modelcontextprotocol/inspector)).
4. **`charles_status`** → confirm `reachable: true` (and `authenticated: true` if you set credentials).
5. **`start_recording` / `stop_recording`** → watch the recording indicator toggle in the Charles UI.
6. **`set_throttling`** → exercise each preset (`56 kbps Modem`, `3G`, `4G`, …) and confirm Charles shows throttling active; then disable.
7. **`set_tool` + `get_tool_status`** → toggle every tool (breakpoints, no-caching, block-cookies, map-remote, map-local, rewrite, black-list, white-list, dns-spoofing, auto-save, client-process) and confirm status round-trips.
8. **`list_requests` / `get_request` / `search_traffic` / `get_session_stats`** → confirm the live session is read, filtered, searched, and that bodies decode correctly (gzip/brotli/base64, JSON pretty-print, binary summaries).
9. **`export_session`** → export to both `chlsj` and `har`; confirm the files are written and well-formed.
10. **`read_session_file`** → read back both exported files and confirm the listings match the live view.
11. **`clear_session`** then **`quit_charles`** (each with `confirm: true`) → confirm Charles clears and exits.
12. **Promote real fixtures**: capture the real control-page HTML into `tests/fixtures/control_page.html`, drop a real `.chlsj` and `.har` export into `tests/fixtures/`, then re-run `cargo test` to lock the schema and endpoint discovery against ground truth.
