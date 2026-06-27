use rusqlite::Connection;

pub const SCHEMA_VERSION: i64 = 1;

const SCHEMA_SQL: &str = r#"
CREATE TABLE captures (
  capture_id    TEXT PRIMARY KEY,
  kind          TEXT NOT NULL,                 -- 'live' | 'file'
  source        TEXT,                          -- file path, or 'live'
  source_key    TEXT,                          -- file: "path:mtime:size"
  generation    INTEGER NOT NULL DEFAULT 0,    -- bumped on every re-ingest
  created_at    TEXT NOT NULL,
  last_used     TEXT NOT NULL,
  entry_count   INTEGER NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX captures_source_key ON captures(source_key) WHERE source_key IS NOT NULL;

CREATE TABLE bodies (
  sha256           TEXT PRIMARY KEY,
  byte_len         INTEGER NOT NULL,
  raw              BLOB NOT NULL,              -- on-the-wire bytes (lazy-decoded on read)
  content_type     TEXT,
  content_encoding TEXT,
  grpc_encoding    TEXT,
  declared_charset TEXT,
  was_base64       INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE entries (
  entry_id          TEXT PRIMARY KEY,
  capture_id        TEXT NOT NULL REFERENCES captures(capture_id) ON DELETE CASCADE,
  seq               INTEGER NOT NULL,          -- 0-based position in the capture (the public index)
  method            TEXT NOT NULL DEFAULT '',
  scheme            TEXT NOT NULL DEFAULT '',
  host              TEXT NOT NULL DEFAULT '',
  path              TEXT NOT NULL DEFAULT '',  -- includes the query string
  url               TEXT NOT NULL DEFAULT '',
  response_status   INTEGER,
  status_text       TEXT,
  mime              TEXT,
  response_size     INTEGER,
  duration_ms       REAL,
  started           TEXT,                      -- RFC3339
  protocol          TEXT,
  client_addr       TEXT,
  remote_addr       TEXT,
  tls_version       TEXT,
  error             TEXT,
  tunnel            INTEGER NOT NULL DEFAULT 0,
  is_websocket      INTEGER NOT NULL DEFAULT 0,
  resource_class    TEXT NOT NULL DEFAULT 'unknown',
  priority          INTEGER NOT NULL DEFAULT 20,
  priority_reasons  TEXT NOT NULL DEFAULT '',  -- comma-joined
  is_error          INTEGER GENERATED ALWAYS AS
                      (error IS NOT NULL OR (response_status IS NOT NULL AND response_status >= 400)) VIRTUAL,
  req_headers_json  TEXT NOT NULL DEFAULT '[]',
  resp_headers_json TEXT,                      -- NULL when no response captured
  req_captured      INTEGER NOT NULL DEFAULT 0,
  resp_captured     INTEGER NOT NULL DEFAULT 0,
  has_response      INTEGER NOT NULL DEFAULT 0,
  req_body_sha      TEXT REFERENCES bodies(sha256),
  resp_body_sha     TEXT REFERENCES bodies(sha256),
  UNIQUE (capture_id, seq)
);
CREATE INDEX entries_browse ON entries(capture_id, priority DESC, seq);
CREATE INDEX entries_host   ON entries(capture_id, host);
CREATE INDEX entries_status ON entries(capture_id, response_status);
CREATE INDEX entries_errors ON entries(capture_id, seq) WHERE is_error = 1;
CREATE INDEX entries_ws     ON entries(capture_id) WHERE is_websocket = 1;

CREATE TABLE ws_frames (
  entry_id  TEXT NOT NULL REFERENCES entries(entry_id) ON DELETE CASCADE,
  seq       INTEGER NOT NULL,
  direction TEXT NOT NULL,                     -- 'sent' | 'received'
  opcode    TEXT NOT NULL,
  body_sha  TEXT REFERENCES bodies(sha256),
  PRIMARY KEY (entry_id, seq)
);

-- Standalone FTS5 over the decoded preview; rowid == entries.rowid for joins.
CREATE VIRTUAL TABLE entries_fts USING fts5(
  url, headers, body_text,
  tokenize='unicode61 remove_diacritics 2'
);
"#;

fn apply_pragmas(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA mmap_size = 268435456;
         PRAGMA auto_vacuum = INCREMENTAL;",
    )
}

pub fn initialize(conn: &Connection) -> rusqlite::Result<()> {
    apply_pragmas(conn)?;
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version == SCHEMA_VERSION && has_core_tables(conn)? {
        return Ok(());
    }
    rebuild(conn)
}

fn has_core_tables(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='entries'",
        [],
        |r| r.get(0),
    )?;
    Ok(n == 1)
}

fn rebuild(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS entries_fts;
         DROP TABLE IF EXISTS ws_frames;
         DROP TABLE IF EXISTS entries;
         DROP TABLE IF EXISTS bodies;
         DROP TABLE IF EXISTS captures;",
    )?;
    conn.execute_batch(SCHEMA_SQL)?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_applies_and_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        initialize(&conn).unwrap();
        conn.execute(
            "INSERT INTO captures(capture_id, kind, created_at, last_used) \
             VALUES ('c1','live','t','t')",
            [],
        )
        .unwrap();
        initialize(&conn).unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM captures", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "re-initialize must not wipe a current-version DB");
    }

    #[test]
    fn generated_is_error_column_and_partial_index_work() {
        let conn = Connection::open_in_memory().unwrap();
        initialize(&conn).unwrap();
        conn.execute(
            "INSERT INTO captures(capture_id, kind, created_at, last_used) VALUES ('c','live','t','t')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO entries(entry_id, capture_id, seq, response_status) VALUES ('e1','c',0,200)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO entries(entry_id, capture_id, seq, response_status) VALUES ('e2','c',1,500)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO entries(entry_id, capture_id, seq, error) VALUES ('e3','c',2,'boom')",
            [],
        )
        .unwrap();
        let errs: i64 = conn
            .query_row(
                "SELECT count(*) FROM entries WHERE capture_id='c' AND is_error=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(errs, 2, "500 and the errored entry are errors; 200 is not");
    }
}
