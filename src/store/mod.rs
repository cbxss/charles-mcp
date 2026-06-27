//! SQLite traffic store: ingest a parsed [`Session`] once, query it many times.
//!
//! The store is the index/cache behind the inspect tools. `list`/`stats`/`search`
//! run as SQL/FTS over the summary columns (never touching bodies), while
//! `get`/`get_all` reconstruct full [`Transaction`]s for one entry on demand so
//! the existing lazy decoder (gzip/brotli/protobuf/gRPC/WebSocket) stays the
//! single source of truth. Bodies are content-addressed (sha256) so identical
//! payloads — the repeated JS/JSON/images that dominate real captures — dedup.
//!
//! All access is serialized through a single connection behind a `Mutex`; the
//! server wraps each call in `spawn_blocking`, so the blocking SQLite calls here
//! never run on the async runtime.

pub mod schema;

use std::path::Path;
use std::sync::Mutex;

use regex::Regex;
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use sha2::{Digest, Sha256};

use crate::error::CharlesError;
use crate::session::classify::classify;
use crate::session::{
    HttpMessage, RawBody, Session, Transaction, TxnSummary, WsDirection, WsMessage, WsOpcode, body,
};

/// A handle to one ingested capture (live snapshot or file).
#[derive(Debug, Clone)]
pub struct CaptureRef {
    pub capture_id: String,
    pub generation: i64,
    pub entry_count: usize,
}

/// One browse/list row (summary columns + the resource-class tag and priority).
#[derive(Debug, Clone)]
pub struct EntryRow {
    pub seq: usize,
    pub method: String,
    pub status: Option<u16>,
    pub host: String,
    pub path: String,
    pub mime: Option<String>,
    pub response_size: Option<u64>,
    pub duration_ms: Option<f64>,
    pub resource_class: String,
    pub priority: i64,
}

impl EntryRow {
    /// A bodyless summary for reuse of the existing table formatter.
    pub fn summary(&self) -> TxnSummary {
        TxnSummary {
            index: self.seq,
            method: self.method.clone(),
            status: self.status,
            host: self.host.clone(),
            path: self.path.clone(),
            mime: self.mime.clone(),
            response_size: self.response_size,
            duration_ms: self.duration_ms,
        }
    }
}

/// Owned (Send) filters for `list`, safe to move into `spawn_blocking`.
#[derive(Debug, Default)]
pub struct StoreFilters {
    pub host: Option<String>,
    pub method: Option<String>,
    pub status: Option<u16>,
    pub mime: Option<String>,
    pub resource_class: Option<String>,
    pub min_priority: Option<i64>,
    /// Applied in Rust (SQLite has no regex): matches against the path+query.
    pub path_regex: Option<Regex>,
    pub limit: usize,
}

pub struct TrafficStore {
    conn: Mutex<Connection>,
}

impl TrafficStore {
    /// Open the store. `None` → an ephemeral in-memory DB (gone on exit);
    /// `Some(path)` → a persistent DB (created if absent).
    pub fn open(path: Option<&Path>) -> Result<Self, CharlesError> {
        let conn = match path {
            Some(p) => Connection::open(p)?,
            None => Connection::open_in_memory()?,
        };
        schema::initialize(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().expect("traffic store mutex poisoned")
    }

    /// Look up a capture by its `source_key` (`path:mtime:size` for files).
    pub fn capture_by_source_key(
        &self,
        source_key: &str,
    ) -> Result<Option<CaptureRef>, CharlesError> {
        let conn = self.lock();
        let row = conn
            .query_row(
                "SELECT capture_id, generation, entry_count FROM captures WHERE source_key = ?1",
                params![source_key],
                |r| {
                    Ok(CaptureRef {
                        capture_id: r.get(0)?,
                        generation: r.get(1)?,
                        entry_count: r.get::<_, i64>(2)? as usize,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Bump a capture's `last_used` timestamp (for LRU eviction of file captures).
    pub fn touch(&self, capture_id: &str) -> Result<(), CharlesError> {
        let conn = self.lock();
        conn.execute(
            "UPDATE captures SET last_used = ?2 WHERE capture_id = ?1",
            params![capture_id, now()],
        )?;
        Ok(())
    }

    /// Replace a capture's contents wholesale (each ingest is a full snapshot —
    /// merging would silently drop legitimately-repeated requests). Bumps the
    /// capture's `generation`. `fts_cap` bounds the decoded text indexed per
    /// message for full-text search.
    pub fn ingest(
        &self,
        capture_id: &str,
        kind: &str,
        source: Option<&str>,
        source_key: Option<&str>,
        session: &Session,
        fts_cap: usize,
    ) -> Result<CaptureRef, CharlesError> {
        let now = now();
        let mut conn = self.lock();
        let tx = conn.transaction()?;

        // Preserve created_at across re-ingest; bump the generation.
        let prev: Option<(i64, String)> = tx
            .query_row(
                "SELECT generation, created_at FROM captures WHERE capture_id = ?1",
                params![capture_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let new_gen = prev.as_ref().map(|(g, _)| g + 1).unwrap_or(0);
        let created_at = prev.map(|(_, c)| c).unwrap_or_else(|| now.clone());

        // Drop the prior snapshot (FTS rows keyed by entries.rowid first, then
        // entries — ws_frames cascade via the FK).
        tx.execute(
            "DELETE FROM entries_fts WHERE rowid IN (SELECT rowid FROM entries WHERE capture_id = ?1)",
            params![capture_id],
        )?;
        tx.execute(
            "DELETE FROM entries WHERE capture_id = ?1",
            params![capture_id],
        )?;

        let entry_count = session.transactions.len();
        tx.execute(
            "INSERT OR REPLACE INTO captures
               (capture_id, kind, source, source_key, generation, created_at, last_used, entry_count)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![capture_id, kind, source, source_key, new_gen, created_at, now, entry_count as i64],
        )?;

        for (seq, t) in session.transactions.iter().enumerate() {
            ingest_entry(&tx, capture_id, seq, t, fts_cap)?;
        }

        // Reclaim bodies no longer referenced by any entry or frame.
        tx.execute(
            "DELETE FROM bodies WHERE sha256 NOT IN (
               SELECT req_body_sha  FROM entries   WHERE req_body_sha  IS NOT NULL
               UNION SELECT resp_body_sha FROM entries   WHERE resp_body_sha IS NOT NULL
               UNION SELECT body_sha      FROM ws_frames WHERE body_sha      IS NOT NULL)",
            [],
        )?;

        tx.commit()?;
        Ok(CaptureRef {
            capture_id: capture_id.to_string(),
            generation: new_gen,
            entry_count,
        })
    }

    /// List/browse entries (summary columns only), ordered by priority then seq.
    /// Returns the (possibly limited) rows and the total match count.
    pub fn list(
        &self,
        capture_id: &str,
        f: &StoreFilters,
    ) -> Result<(Vec<EntryRow>, usize), CharlesError> {
        let mut sql = String::from(
            "SELECT seq, method, response_status, host, path, mime, response_size, duration_ms, \
             resource_class, priority FROM entries WHERE capture_id = ?1",
        );
        let mut p: Vec<Value> = vec![Value::Text(capture_id.to_string())];
        if let Some(h) = &f.host {
            p.push(Value::Text(format!("%{}%", h.to_lowercase())));
            sql.push_str(&format!(" AND lower(host) LIKE ?{}", p.len()));
        }
        if let Some(m) = &f.method {
            p.push(Value::Text(m.to_uppercase()));
            sql.push_str(&format!(" AND upper(method) = ?{}", p.len()));
        }
        if let Some(s) = f.status {
            p.push(Value::Integer(s as i64));
            sql.push_str(&format!(" AND response_status = ?{}", p.len()));
        }
        if let Some(m) = &f.mime {
            p.push(Value::Text(format!("%{}%", m.to_lowercase())));
            sql.push_str(&format!(" AND lower(mime) LIKE ?{}", p.len()));
        }
        if let Some(c) = &f.resource_class {
            p.push(Value::Text(c.clone()));
            sql.push_str(&format!(" AND resource_class = ?{}", p.len()));
        }
        if let Some(mp) = f.min_priority {
            p.push(Value::Integer(mp));
            sql.push_str(&format!(" AND priority >= ?{}", p.len()));
        }
        sql.push_str(" ORDER BY priority DESC, seq");

        let conn = self.lock();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(p.iter()), row_to_entry)?;

        let mut out = Vec::new();
        for r in rows {
            let row = r?;
            if let Some(re) = &f.path_regex
                && !re.is_match(&row.path)
            {
                continue;
            }
            out.push(row);
        }
        let total = out.len();
        if out.len() > f.limit {
            out.truncate(f.limit);
        }
        Ok((out, total))
    }

    /// Aggregate statistics for a capture, built entirely in SQL.
    pub fn stats(&self, capture_id: &str) -> Result<crate::tools::inspect::Stats, CharlesError> {
        let conn = self.lock();
        let cid = params![capture_id];

        let total: i64 = conn.query_row(
            "SELECT count(*) FROM entries WHERE capture_id=?1",
            cid,
            |r| r.get(0),
        )?;
        let errors: i64 = conn.query_row(
            "SELECT count(*) FROM entries WHERE capture_id=?1 AND is_error=1",
            cid,
            |r| r.get(0),
        )?;
        let total_response_bytes: i64 = conn.query_row(
            "SELECT COALESCE(SUM(response_size),0) FROM entries WHERE capture_id=?1",
            cid,
            |r| r.get(0),
        )?;

        let by_host = group_count(
            &conn,
            "SELECT host, count(*) c FROM entries WHERE capture_id=?1 GROUP BY host \
             ORDER BY c DESC, host",
            capture_id,
        )?;
        let by_status = group_count(
            &conn,
            "SELECT COALESCE(CAST(response_status AS TEXT),'(none)') s, count(*) c FROM entries \
             WHERE capture_id=?1 GROUP BY response_status ORDER BY c DESC, s",
            capture_id,
        )?;
        let by_mime = group_count(
            &conn,
            "SELECT COALESCE(mime,'(none)') m, count(*) c FROM entries WHERE capture_id=?1 \
             GROUP BY mime ORDER BY c DESC, m",
            capture_id,
        )?;

        let mut stmt = conn.prepare(
            "SELECT seq, url, duration_ms FROM entries WHERE capture_id=?1 AND duration_ms IS NOT NULL \
             ORDER BY duration_ms DESC LIMIT 5",
        )?;
        let slowest = stmt
            .query_map(cid, |r| {
                Ok((
                    r.get::<_, i64>(0)? as usize,
                    r.get::<_, String>(1)?,
                    r.get::<_, f64>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(crate::tools::inspect::Stats {
            total: total as usize,
            errors: errors as usize,
            total_response_bytes: total_response_bytes as u64,
            by_host,
            by_status,
            by_mime,
            slowest,
        })
    }

    /// Full-text search via FTS5 (ranked by bm25). Returns hits as (seq, snippet).
    pub fn search_fts(
        &self,
        capture_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(usize, String)>, CharlesError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT e.seq, \
                snippet(entries_fts, 2, '', '', '…', 12) AS body_snip, \
                e.url \
             FROM entries_fts f JOIN entries e ON e.rowid = f.rowid \
             WHERE e.capture_id = ?1 AND entries_fts MATCH ?2 \
             ORDER BY bm25(entries_fts) LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![capture_id, query, limit as i64], |r| {
            let seq = r.get::<_, i64>(0)? as usize;
            let snip: String = r.get(1)?;
            let url: String = r.get(2)?;
            let snippet = if snip.trim().is_empty() { url } else { snip };
            Ok((seq, snippet))
        })?;
        rows.map(|r| r.map_err(CharlesError::from)).collect()
    }

    /// Reconstruct one full [`Transaction`] (headers + bodies + WS frames) by its
    /// position (`seq`) in the capture, for `get_request` / replay.
    pub fn get(&self, capture_id: &str, seq: usize) -> Result<Option<Transaction>, CharlesError> {
        let conn = self.lock();
        load_transaction(&conn, capture_id, seq)
    }

    /// Reconstruct every transaction in a capture (used by the regex search
    /// fallback, which must scan decoded bodies). Ordered by seq.
    pub fn get_all(&self, capture_id: &str) -> Result<Vec<Transaction>, CharlesError> {
        let conn = self.lock();
        let seqs: Vec<usize> = {
            let mut stmt =
                conn.prepare("SELECT seq FROM entries WHERE capture_id=?1 ORDER BY seq")?;
            let rows = stmt.query_map(params![capture_id], |r| Ok(r.get::<_, i64>(0)? as usize))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let mut out = Vec::with_capacity(seqs.len());
        for seq in seqs {
            if let Some(t) = load_transaction(&conn, capture_id, seq)? {
                out.push(t);
            }
        }
        Ok(out)
    }

    /// Evict least-recently-used FILE captures beyond `keep` (live is untouched).
    pub fn evict_file_captures(&self, keep: usize) -> Result<(), CharlesError> {
        let conn = self.lock();
        // Capture ids to drop: file captures sorted oldest-first, skipping `keep`.
        let victims: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT capture_id FROM captures WHERE kind='file' ORDER BY last_used DESC LIMIT -1 OFFSET ?1",
            )?;
            let rows = stmt.query_map(params![keep as i64], |r| r.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for cid in &victims {
            drop_capture(&conn, cid)?;
        }
        if !victims.is_empty() {
            sweep_orphan_bodies(&conn)?;
        }
        Ok(())
    }

    /// Drop everything and reclaim space.
    pub fn reset(&self) -> Result<(), CharlesError> {
        let conn = self.lock();
        conn.execute("DELETE FROM entries_fts", [])?;
        conn.execute("DELETE FROM entries", [])?;
        conn.execute("DELETE FROM bodies", [])?;
        conn.execute("DELETE FROM captures", [])?;
        // Best-effort space reclaim (a no-op unless auto_vacuum=INCREMENTAL took).
        let _ = conn.execute("PRAGMA incremental_vacuum", []);
        Ok(())
    }
}

// ---- free helpers (operate on a borrowed connection / transaction) ----------

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn sha_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

fn row_to_entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<EntryRow> {
    Ok(EntryRow {
        seq: r.get::<_, i64>(0)? as usize,
        method: r.get(1)?,
        status: r.get::<_, Option<i64>>(2)?.map(|s| s as u16),
        host: r.get(3)?,
        path: r.get(4)?,
        mime: r.get(5)?,
        response_size: r.get::<_, Option<i64>>(6)?.map(|s| s as u64),
        duration_ms: r.get(7)?,
        resource_class: r.get(8)?,
        priority: r.get(9)?,
    })
}

fn group_count(
    conn: &Connection,
    sql: &str,
    capture_id: &str,
) -> Result<Vec<(String, usize)>, CharlesError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![capture_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as usize))
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Content-address and upsert a captured body; returns its sha (None if absent).
fn store_body(
    tx: &rusqlite::Transaction<'_>,
    raw: &RawBody,
) -> Result<Option<String>, CharlesError> {
    if !raw.captured || raw.bytes.is_empty() {
        return Ok(None);
    }
    let sha = sha_hex(&raw.bytes);
    tx.execute(
        "INSERT OR IGNORE INTO bodies
           (sha256, byte_len, raw, content_type, content_encoding, grpc_encoding, declared_charset, was_base64)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            sha,
            raw.bytes.len() as i64,
            raw.bytes,
            raw.content_type,
            raw.content_encoding,
            raw.grpc_encoding,
            raw.declared_charset,
            raw.was_base64_wrapped as i64,
        ],
    )?;
    Ok(Some(sha))
}

fn ingest_entry(
    tx: &rusqlite::Transaction<'_>,
    capture_id: &str,
    seq: usize,
    t: &Transaction,
    fts_cap: usize,
) -> Result<(), CharlesError> {
    let c = classify(t);
    let entry_id = format!("{capture_id}#{seq}");

    let req_sha = store_body(tx, &t.request.raw)?;
    let resp_sha = match &t.response {
        Some(r) => store_body(tx, &r.raw)?,
        None => None,
    };

    let req_headers = serde_json::to_string(&t.request.headers)?;
    let resp_headers = match &t.response {
        Some(r) => Some(serde_json::to_string(&r.headers)?),
        None => None,
    };

    tx.execute(
        "INSERT INTO entries
           (entry_id, capture_id, seq, method, scheme, host, path, url, response_status, status_text,
            mime, response_size, duration_ms, started, protocol, client_addr, remote_addr, tls_version,
            error, tunnel, is_websocket, resource_class, priority, priority_reasons,
            req_headers_json, resp_headers_json, req_captured, resp_captured, has_response,
            req_body_sha, resp_body_sha)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,
                 ?24,?25,?26,?27,?28,?29,?30,?31)",
        params![
            entry_id,
            capture_id,
            seq as i64,
            t.method,
            t.scheme,
            t.host,
            t.path,
            t.url,
            t.status.map(|s| s as i64),
            t.status_text,
            t.mime,
            t.response_size.map(|s| s as i64),
            t.duration_ms,
            t.started.map(|d| d.to_rfc3339()),
            t.protocol,
            t.client_addr,
            t.remote_addr,
            t.tls_version,
            t.error,
            t.tunnel as i64,
            t.websocket.is_some() as i64,
            c.class.as_str(),
            c.priority,
            c.reasons.join(","),
            req_headers,
            resp_headers,
            t.request.raw.captured as i64,
            t.response.as_ref().map(|r| r.raw.captured).unwrap_or(false) as i64,
            t.response.is_some() as i64,
            req_sha,
            resp_sha,
        ],
    )?;
    let rowid = tx.last_insert_rowid();

    // WebSocket frames out-of-row (a connection can carry thousands).
    if let Some(frames) = &t.websocket {
        for (fseq, m) in frames.iter().enumerate() {
            let body_sha = store_body(tx, &m.payload)?;
            tx.execute(
                "INSERT INTO ws_frames (entry_id, seq, direction, opcode, body_sha) VALUES (?1,?2,?3,?4,?5)",
                params![entry_id, fseq as i64, direction_str(m.direction), opcode_str(m.opcode), body_sha],
            )?;
        }
    }

    // FTS row: url + headers + the decode-once body preview (incl. protobuf tree).
    let (headers_text, body_text) = fts_text(t, fts_cap);
    tx.execute(
        "INSERT INTO entries_fts (rowid, url, headers, body_text) VALUES (?1,?2,?3,?4)",
        params![rowid, t.url, headers_text, body_text],
    )?;
    Ok(())
}

/// Build the searchable text for one transaction: joined header lines and the
/// decoded request+response bodies (text or protobuf tree), each capped.
fn fts_text(t: &Transaction, cap: usize) -> (String, String) {
    let mut headers = String::new();
    for (k, v) in &t.request.headers {
        headers.push_str(k);
        headers.push_str(": ");
        headers.push_str(v);
        headers.push('\n');
    }
    if let Some(r) = &t.response {
        for (k, v) in &r.headers {
            headers.push_str(k);
            headers.push_str(": ");
            headers.push_str(v);
            headers.push('\n');
        }
    }

    let mut body_text = String::new();
    let mut push_body = |raw: &RawBody| {
        match body::decode(raw, cap) {
            crate::session::Body::Text { text, .. } => body_text.push_str(&text),
            crate::session::Body::Protobuf { tree, .. } => body_text.push_str(&tree),
            _ => {}
        }
        body_text.push('\n');
    };
    push_body(&t.request.raw);
    if let Some(r) = &t.response {
        push_body(&r.raw);
    }
    (headers, body_text)
}

fn load_transaction(
    conn: &Connection,
    capture_id: &str,
    seq: usize,
) -> Result<Option<Transaction>, CharlesError> {
    let row = conn
        .query_row(
            "SELECT entry_id, method, scheme, host, path, url, response_status, status_text, mime, \
                    response_size, duration_ms, started, protocol, client_addr, remote_addr, \
                    tls_version, error, tunnel, is_websocket, req_headers_json, resp_headers_json, \
                    req_captured, resp_captured, has_response, req_body_sha, resp_body_sha \
             FROM entries WHERE capture_id=?1 AND seq=?2",
            params![capture_id, seq as i64],
            |r| {
                Ok(LoadedEntry {
                    entry_id: r.get(0)?,
                    method: r.get(1)?,
                    scheme: r.get(2)?,
                    host: r.get(3)?,
                    path: r.get(4)?,
                    url: r.get(5)?,
                    status: r.get::<_, Option<i64>>(6)?.map(|s| s as u16),
                    status_text: r.get(7)?,
                    mime: r.get(8)?,
                    response_size: r.get::<_, Option<i64>>(9)?.map(|s| s as u64),
                    duration_ms: r.get(10)?,
                    started: r.get(11)?,
                    protocol: r.get(12)?,
                    client_addr: r.get(13)?,
                    remote_addr: r.get(14)?,
                    tls_version: r.get(15)?,
                    error: r.get(16)?,
                    tunnel: r.get::<_, i64>(17)? != 0,
                    is_websocket: r.get::<_, i64>(18)? != 0,
                    req_headers_json: r.get(19)?,
                    resp_headers_json: r.get(20)?,
                    req_captured: r.get::<_, i64>(21)? != 0,
                    resp_captured: r.get::<_, i64>(22)? != 0,
                    has_response: r.get::<_, i64>(23)? != 0,
                    req_body_sha: r.get(24)?,
                    resp_body_sha: r.get(25)?,
                })
            },
        )
        .optional()?;
    let Some(e) = row else { return Ok(None) };

    let request = HttpMessage {
        headers: parse_headers(&e.req_headers_json)?,
        raw: load_body(conn, e.req_body_sha.as_deref(), e.req_captured)?,
    };
    let response = if e.has_response {
        Some(HttpMessage {
            headers: e
                .resp_headers_json
                .as_deref()
                .map(parse_headers)
                .transpose()?
                .unwrap_or_default(),
            raw: load_body(conn, e.resp_body_sha.as_deref(), e.resp_captured)?,
        })
    } else {
        None
    };

    let websocket = if e.is_websocket {
        Some(load_ws_frames(conn, &e.entry_id)?)
    } else {
        None
    };

    let started = e.started.as_deref().and_then(|s| {
        chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.with_timezone(&chrono::Utc))
    });

    Ok(Some(Transaction {
        index: seq,
        started,
        duration_ms: e.duration_ms,
        scheme: e.scheme,
        host: e.host,
        method: e.method,
        path: e.path,
        url: e.url,
        status: e.status,
        status_text: e.status_text,
        mime: e.mime,
        response_size: e.response_size,
        protocol: e.protocol,
        client_addr: e.client_addr,
        remote_addr: e.remote_addr,
        tls_version: e.tls_version,
        tunnel: e.tunnel,
        error: e.error,
        request,
        response,
        websocket,
    }))
}

struct LoadedEntry {
    entry_id: String,
    method: String,
    scheme: String,
    host: String,
    path: String,
    url: String,
    status: Option<u16>,
    status_text: Option<String>,
    mime: Option<String>,
    response_size: Option<u64>,
    duration_ms: Option<f64>,
    started: Option<String>,
    protocol: Option<String>,
    client_addr: Option<String>,
    remote_addr: Option<String>,
    tls_version: Option<String>,
    error: Option<String>,
    tunnel: bool,
    is_websocket: bool,
    req_headers_json: String,
    resp_headers_json: Option<String>,
    req_captured: bool,
    resp_captured: bool,
    has_response: bool,
    req_body_sha: Option<String>,
    resp_body_sha: Option<String>,
}

fn parse_headers(json: &str) -> Result<Vec<(String, String)>, CharlesError> {
    Ok(serde_json::from_str(json)?)
}

/// Rebuild a [`RawBody`] from a stored body row (or an empty captured/uncaptured
/// body when there is no row).
fn load_body(
    conn: &Connection,
    sha: Option<&str>,
    captured: bool,
) -> Result<RawBody, CharlesError> {
    let Some(sha) = sha else {
        return Ok(RawBody {
            captured,
            ..Default::default()
        });
    };
    let raw = conn
        .query_row(
            "SELECT raw, content_type, content_encoding, grpc_encoding, declared_charset, was_base64 \
             FROM bodies WHERE sha256=?1",
            params![sha],
            |r| {
                Ok(RawBody {
                    bytes: r.get(0)?,
                    content_type: r.get(1)?,
                    content_encoding: r.get(2)?,
                    grpc_encoding: r.get(3)?,
                    declared_charset: r.get(4)?,
                    was_base64_wrapped: r.get::<_, i64>(5)? != 0,
                    captured: true,
                })
            },
        )
        .optional()?;
    Ok(raw.unwrap_or(RawBody {
        captured,
        ..Default::default()
    }))
}

fn load_ws_frames(conn: &Connection, entry_id: &str) -> Result<Vec<WsMessage>, CharlesError> {
    let mut stmt = conn.prepare(
        "SELECT direction, opcode, body_sha FROM ws_frames WHERE entry_id=?1 ORDER BY seq",
    )?;
    let rows = stmt.query_map(params![entry_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut frames = Vec::new();
    for r in rows {
        let (dir, opcode, sha) = r?;
        frames.push(WsMessage {
            direction: parse_direction(&dir),
            opcode: parse_opcode(&opcode),
            payload: load_body(conn, sha.as_deref(), true)?,
        });
    }
    Ok(frames)
}

fn drop_capture(conn: &Connection, capture_id: &str) -> Result<(), CharlesError> {
    conn.execute(
        "DELETE FROM entries_fts WHERE rowid IN (SELECT rowid FROM entries WHERE capture_id=?1)",
        params![capture_id],
    )?;
    conn.execute(
        "DELETE FROM entries WHERE capture_id=?1",
        params![capture_id],
    )?;
    conn.execute(
        "DELETE FROM captures WHERE capture_id=?1",
        params![capture_id],
    )?;
    Ok(())
}

fn sweep_orphan_bodies(conn: &Connection) -> Result<(), CharlesError> {
    conn.execute(
        "DELETE FROM bodies WHERE sha256 NOT IN (
           SELECT req_body_sha  FROM entries   WHERE req_body_sha  IS NOT NULL
           UNION SELECT resp_body_sha FROM entries   WHERE resp_body_sha IS NOT NULL
           UNION SELECT body_sha      FROM ws_frames WHERE body_sha      IS NOT NULL)",
        [],
    )?;
    Ok(())
}

fn direction_str(d: WsDirection) -> &'static str {
    match d {
        WsDirection::Sent => "sent",
        WsDirection::Received => "received",
    }
}

fn parse_direction(s: &str) -> WsDirection {
    match s {
        "sent" => WsDirection::Sent,
        _ => WsDirection::Received,
    }
}

fn opcode_str(op: WsOpcode) -> String {
    match op {
        WsOpcode::Text => "text".to_string(),
        WsOpcode::Binary => "binary".to_string(),
        WsOpcode::Ping => "ping".to_string(),
        WsOpcode::Pong => "pong".to_string(),
        WsOpcode::Close => "close".to_string(),
        WsOpcode::Other(b) => format!("other:{b}"),
    }
}

fn parse_opcode(s: &str) -> WsOpcode {
    match s {
        "text" => WsOpcode::Text,
        "binary" => WsOpcode::Binary,
        "ping" => WsOpcode::Ping,
        "pong" => WsOpcode::Pong,
        "close" => WsOpcode::Close,
        other => other
            .strip_prefix("other:")
            .and_then(|n| n.parse().ok())
            .map(WsOpcode::Other)
            .unwrap_or(WsOpcode::Other(0)),
    }
}
