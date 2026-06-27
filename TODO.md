# TODO — remaining work (mostly verified live)

Most of this list is now **confirmed against a real Charles 5**: the read path
(`/session/export-json`), the real session endpoints (`session/export-{json,har,
xml,csv}`, `session/download`, `session/clear`, `quit`), the control verbs
(recording / throttling / tools + `get_tool_status`), `charles convert`,
`replay_request`, anonymous-auth reporting, and the schema/decoders/store. What
remains is a handful of edge conditions and feature work, marked below.

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

- [x] **Session-state enum confirmed: `EXCEPTION` is the failure state** (the 51 SSL-handshake failures — device didn't trust the Charles cert / pinning). `error` is set from `errorMessage` and rendered; the bogus default `status: 200` on a failed connection is now nulled. `tunnel` (SSL-Proxying-OFF passthrough) is a *different* case — handled + synthetic-tested; a real `tunnel:true` example would be nice-to-have but isn't required.

- [x] **Timing fields confirmed**: `durations.total` (+ full `dns/connect/ssl/request/response/latency` breakdown) and `times.start` (ISO-8601). `duration_ms` and `slowest` populate correctly. (Optional future: surface the per-phase breakdown in `get_request`.)

- [x] **WebSocket + gRPC + protobuf — DONE** (built against the real capture). WS frames are raw RFC 6455 in the request/response body (`webSocket: true` flag); parsed + unmasked + reassembled → `get_websocket_messages`, with protobuf-over-WS decoded. Schemaless protobuf + gRPC framing + optional `.proto` (`--proto-dir`). Tesla signaling and piesocket verified end-to-end.
  - [ ] **SSE** (`text/event-stream`) still renders as one text blob — split into events if a use-case needs it.

## P0 — endpoint discovery (live read/clear/quit ride guessed paths)

- [x] **Captured the real `control.charles` pages and validated `discover_from_html`.** The real root page is just nav links (`throttling/ recording/ tools/ session/ quit`) — the only endpoint it exposes is `quit`; the session ops live on the `session/` subpage as relative links. Both captured as `tests/fixtures/control_page.html` + `control_session_page.html`; `tests/discovery.rs` now asserts the real behavior (root → quit; subpage → clear/export/download with relative, prefix-less paths).

- [x] **Locked the real session endpoint paths** (confirmed live + wired as primary candidates).
  - Confirmed `200` against real Charles 5: `session/export-json` (= our chlsj), `session/export-har`, `session/export-xml`, `session/export-csv`, `session/download` (native), `session/clear`, `quit`. The older `session/export-session?format=…` guesses all `404`. `candidate_export_paths` now maps each format to its real `session/export-*` path first; `download_native` leads with `session/download`; `try_clear_candidates` leads with `session/clear`.
  - Files: `candidate_export_paths`, `real_export_path`, `download_native`, `try_clear_candidates` in `src/web/live.rs`.
  - Live-verified: `export_session` chlsj + har round-trip (export → read_session_file), `clear_session`. Not yet: `quit_charles` (would close Charles).

## P1 — robustness / behavior to verify live

- [x] **`charles convert` invocation — works** with the default `/Applications/Charles.app/Contents/MacOS/Charles convert in out`, **including while Charles is already running** (no single-instance collision); the gated `convert_real.rs` tests pass against the live install. Not exercised: a trial/unregistered copy's license nag (still relies on `--convert-timeout-ms`).

- [x] **Control verbs end-to-end — confirmed live.** `start_recording`/`stop_recording`, `set_throttling` activate/deactivate, and `set_tool` toggle all return success against real Charles 5. **Bug fix:** the `set_tool`/`get_tool_status` segments `black-list`/`white-list` were wrong (they hit a 404 and never worked) — renamed to the real Charles 5 `block-list`/`allow-list`.
  - Files: `src/web/control.rs`.

- [x] **`get_tool_status` parsing — confirmed live.** Round-tripped `block-cookies` disabled → enabled → disabled and read the `Status:` marker back correctly each time (proves `set_tool` takes effect *and* the parse heuristic holds).
  - Files: `get_tool_status` in `src/web/control.rs`.

- [x] **Throttling presets — DONE.** `set_throttling` now scrapes the configured presets from the `throttling/` page and **validates** the `preset` against them (an unknown name returns an error listing the real presets instead of silently succeeding); new read tool **`get_throttling`** reports whether throttling is active plus the available presets.
  - Files: `set_throttling` in `src/web/control.rs`, description in `src/server.rs`.

- [x] **Auth realm / anonymous — DONE.** Anonymous-access reporting confirmed live (`charles_status` → "anonymous access"); the basic-auth **realm** is now parsed from `WWW-Authenticate` on a 401 and surfaced in `charles_status`.
  - Files: `WebClient::status`, `raw_request`/`send_control` in `src/web/{mod,live}.rs`.

- [ ] **Performance on a real (hundreds-of-MB) session.** Partly addressed: the SQLite store ingests once and queries from SQL/FTS (no re-parse per call), `--export-timeout-ms` separates the whole-session read from per-request timeouts, and the server's own `control.charles` reads are dropped from the session by default (`--include-control-traffic` to keep them) — confirmed live that repeated `/session/export-json` reads otherwise nest and the session balloons exponentially (66 KB → 80 MB over a few reads). Still to do live: measure export+convert cost, tune `--cache-ttl-ms`, check for a delta/`since` export param, consider a streaming parse instead of whole-session-in-RAM, and document the Charles-side recording filter that excludes `control.charles` at the source.
  - Files: `fetch_live_session` in `src/web/live.rs`, `resolve_session` in `src/server.rs`.

## P2 — capabilities a Charles power user expects (feature work, not bugs)

- [~] Respond to **breakpoints** (intercept → edit → Execute/Abort) — **won't-do / infeasible via the supported API.** Confirmed live: the Charles Web Interface exposes no breakpoint queue/response (only the tool's Status + enable/disable), so this can't be done here. Mitigation shipped: `set_tool(breakpoints, enable)` now requires `confirm: true` (it hangs matching traffic with no way to release it), and the tool description says so.
- [x] **Compose / Repeat / Repeat Advanced** — delivered by `replay_request` (re-issue a captured request with query/header/json/body overrides, mutating-gated) and **live-validated** (replayed a real GET → 200 with baseline diff). (Still open: "get request as curl/raw".)
- [~] **Rule management** for Map Local / Map Remote / Rewrite / Breakpoints — **won't-do / infeasible via the supported API.** Confirmed live: the Web Interface exposes no rule CRUD (only each tool's Status + enable/disable), so the master-switch toggles stay no-ops without GUI rules. Tool descriptions now say rules are GUI-only.
- [x] **Live tail / Focus / per-host watch — DONE** (`list_requests(only_new: true)` + the existing `host`/`resource_class`/`min_priority` filters: a server-side watermark returns just the requests that arrived since your last call). Caveat: it still re-exports the whole session under the hood, since Charles has no delta/`since` export endpoint.

---

_When a fixture is promoted from a real capture, delete the corresponding "provisional" caveat in `README.md` and the fixture header comment._
