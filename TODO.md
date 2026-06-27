# TODO — fixes that need a live Charles 5

These are the items from the adversarial power-user review that **cannot be
verified or finished without a running Charles 5 install**. The no-live-Charles
fixes are already done (see commit `3b30571`). Everything below is blocked on
ground truth from a real instance.

## How to unblock all of this (do once)

1. Install Charles 5, enable **Proxy → Web Interface Settings** (set a user/pass to test auth too).
2. Enable **Proxy → SSL Proxying** for at least one host so you capture both decrypted and tunneled HTTPS.
3. Drive traffic through `127.0.0.1:8888`: plain HTTP, decrypted HTTPS, an undecrypted HTTPS host, a gzip/brotli response, a binary/image, a failing request, a WebSocket.
4. Capture ground-truth artifacts:
   - `curl -x http://127.0.0.1:8888 http://control.charles/ -o control_page.html` — the **real** control page.
   - From the UI, export the session as **`.chlsj`** and **`.har`**, and **Save** the native `.chls`.
   - Note the exact form actions / link hrefs in `control_page.html` for export / download / clear / quit.
5. Drop the real captures into `tests/fixtures/` (replacing the provisional ones) and re-run `cargo test`. Each item below says what to check.

---

## P0 — correctness (the tool can lie until these are confirmed)

- [x] **Validated the `.chlsj` schema against a real 27 MB export.** Confirmed top-level array; `mimeType`/`charset`/`contentEncoding` are on the **message** (not the body) — fixed; `response.status` is the int code; `body.encoded`+`encoding:"base64"` for binary; TLS is **`ssl.protocol`** (was reading a nonexistent `tlsVersion`) — fixed; `errorMessage` carries the real failure text — now used; `times`/`durations` confirmed (see below). Schema-mismatch guard stays quiet on real data.

- [x] **Session-state enum confirmed: `EXCEPTION` is the failure state** (the 51 SSL-handshake failures). `is_failed_state` matches it; `error` is set from `errorMessage`. `tunnel` field present and parsed. (Note: the capture had SSL Proxying ON for its hosts, so no `tunnel:true` example — still want one capture with SSL Proxying OFF to exercise the tunnel-rendering path.)

- [x] **Timing fields confirmed**: `durations.total` (+ full `dns/connect/ssl/request/response/latency` breakdown) and `times.start` (ISO-8601). `duration_ms` and `slowest` populate correctly. (Optional future: surface the per-phase breakdown in `get_request`.)

- [x] **WebSocket + gRPC + protobuf — DONE** (built against the real capture). WS frames are raw RFC 6455 in the request/response body (`webSocket: true` flag); parsed + unmasked + reassembled → `get_websocket_messages`, with protobuf-over-WS decoded. Schemaless protobuf + gRPC framing + optional `.proto` (`--proto-dir`). Tesla signaling and piesocket verified end-to-end.
  - [ ] **SSE** (`text/event-stream`) still renders as one text blob — split into events if a use-case needs it.

## P0 — endpoint discovery (live read/clear/quit ride guessed paths)

- [ ] **Capture the real `control.charles` page and validate `discover_from_html`**.
  - The provisional `tests/fixtures/control_page.html` is hand-authored. Replace with a real capture and confirm `discover_from_html` extracts export/download/clear/quit (method, path, format `<select>`).
  - Files: `src/web/discovery.rs`, `tests/discovery.rs`, `tests/fixtures/control_page.html`.

- [ ] **Lock the real session endpoint paths** (replace the invented candidates).
  - Only `session/download` (native `.chls`) is confirmed. `session/export-session?format=…`, `session/clear-session`, `quit`/`application/quit`/`shutdown` are guesses.
  - Files: `candidate_export_paths`, `download_native`, `try_clear_candidates`, `quit_charles` in `src/web/live.rs`.
  - Done when: `export_session` (chlsj+har), `clear_session`, `quit_charles` work against a real install via discovery (not the convert fallback).

## P1 — robustness / behavior to verify live

- [ ] **`charles convert` invocation.** Confirm whether `/Applications/Charles.app/Contents/MacOS/Charles convert in out` actually works, or whether the `charles` CLI wrapper (Help → Install Command Line Tools) is required. Check behavior while Charles is **already running** (single-instance collision) and on a trial/unregistered copy (license nag → relies on `--convert-timeout-ms`).
  - Files: `src/session/convert.rs`, `charles_bin` default in `src/config.rs`. Consider defaulting `--charles-bin` to the `charles` CLI if that's the supported path.

- [ ] **Control verbs end-to-end.** Recording start/stop, throttling activate/deactivate, every `tools/<seg>/enable|disable` — confirm each actually takes effect (they match the reference but were never executed live).
  - Files: `src/web/control.rs`.

- [ ] **`get_tool_status` parsing** against the real tool page. Confirm the `Status: Enabled/Disabled` marker text and that the 40-char window heuristic holds.
  - Files: `get_tool_status` in `src/web/control.rs`.

- [ ] **Throttling presets.** Confirm the activate response; consider reading back the active preset / enumerating configured presets so `set_throttling` can validate the name instead of silently succeeding.
  - Files: `set_throttling` in `src/web/control.rs`, description in `src/server.rs`.

- [ ] **Auth realm / anonymous.** Verify basic-auth realm and that anonymous-allowed vs authenticated is reported correctly by `charles_status`.
  - Files: `WebClient::status`, `raw_request`/`send_control` in `src/web/{mod,live}.rs`.

- [ ] **Performance on a real (hundreds-of-MB) session.** Measure export+convert cost and tune `--cache-ttl-ms` / `--timeout-ms`. Investigate whether the export endpoint supports a delta/`since` param to avoid pulling the whole session each refresh; consider streaming parse instead of whole-session-in-RAM.
  - Files: `fetch_live_session` in `src/web/live.rs`, `resolve_session` in `src/server.rs`.

## P2 — capabilities a Charles power user expects (feature work, not bugs)

- [ ] Respond to **breakpoints** (intercept → edit → Execute/Abort). Today enabling breakpoints can hang traffic with no way to release it. The Web Interface exposes no breakpoint-response endpoint, so this likely needs a different integration path.
- [ ] **Compose / Repeat / Repeat Advanced** — replay or craft a request and resend (Charles's killer API workflow). At minimum, "get request as curl/raw".
- [ ] **Rule management** for Map Local / Map Remote / Rewrite / Breakpoints (the master-switch toggles are no-ops without rules).
- [ ] **Live tail / Focus / per-host watch** without re-exporting the whole session.

---

_When a fixture is promoted from a real capture, delete the corresponding "provisional" caveat in `README.md` and the fixture header comment._
