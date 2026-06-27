//! Read session files from disk, shelling out to `charles convert` for `.chls`.

use std::path::Path;

use tokio::process::Command;

use super::{Session, SessionSource, looks_like_schema_mismatch, sniff};
use crate::config::Config;
use crate::error::CharlesError;

/// Parse a `.chls` / `.har` / `.chlsj` file into a [`Session`].
pub async fn read_session_file(cfg: &Config, path: &Path) -> Result<Session, CharlesError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let bytes = if ext == "chls" || ext == "chlz" {
        // Native Charles formats (.chls / compressed .chlz) → convert to chlsj.
        convert_file(cfg, path, "chlsj").await?
    } else {
        tokio::fs::read(path).await?
    };

    let transactions = sniff::parse_bytes(bytes)?;
    // Turn a silent schema mismatch (parsed rows, all fields empty) into a
    // clear error instead of confidently returning garbage.
    if looks_like_schema_mismatch(&transactions) {
        return Err(CharlesError::Parse(format!(
            "parsed {} entries but every host/method is empty — this file's schema does not \
             match what the parser expects (wrong format, or an unsupported Charles version)",
            transactions.len()
        )));
    }
    Ok(Session {
        source: SessionSource::File(path.to_path_buf()),
        transactions,
    })
}

/// Convert in-memory session bytes (e.g. a downloaded native `.chls`) to another
/// format by writing them to a temp file and shelling out to Charles.
pub async fn convert_bytes(
    cfg: &Config,
    bytes: &[u8],
    in_ext: &str,
    out_ext: &str,
) -> Result<Vec<u8>, CharlesError> {
    let infile = tempfile::Builder::new()
        .suffix(&format!(".{in_ext}"))
        .tempfile()?;
    tokio::fs::write(infile.path(), bytes).await?;
    convert_file(cfg, infile.path(), out_ext).await
}

/// Convert a session file to `out_ext` bytes via the Charles binary. Charles
/// picks the format from the output suffix. Bounded by `convert_timeout` so a
/// license/GUI prompt can't hang the tool forever.
pub async fn convert_file(
    cfg: &Config,
    infile: &Path,
    out_ext: &str,
) -> Result<Vec<u8>, CharlesError> {
    if !cfg.charles_bin.exists() {
        return Err(CharlesError::CharlesBinMissing(cfg.charles_bin.clone()));
    }
    // `charles convert` refuses to overwrite an existing output file, so the
    // output path must NOT exist yet — use a temp dir + a fresh name inside it
    // (not `tempfile()`, which creates the file).
    let dir = tempfile::tempdir()?;
    let out_path = dir.path().join(format!("session.{out_ext}"));

    let run = Command::new(&cfg.charles_bin)
        .arg("convert")
        .arg(infile)
        .arg(&out_path)
        .output();

    let result = tokio::time::timeout(cfg.convert_timeout(), run)
        .await
        .map_err(|_| {
            CharlesError::ConvertFailed(format!(
                "`charles convert` timed out after {} ms — is Charles showing a license/GUI \
                 prompt, or is another Charles instance holding it? (raise --convert-timeout-ms)",
                cfg.convert_timeout_ms
            ))
        })??;

    if !result.status.success() {
        return Err(CharlesError::ConvertFailed(
            String::from_utf8_lossy(&result.stderr).trim().to_string(),
        ));
    }
    let bytes = tokio::fs::read(&out_path).await?;
    if bytes.is_empty() {
        return Err(CharlesError::ConvertFailed(
            "`charles convert` produced no output (the binary may have launched the GUI instead \
             of converting; ensure command-line tools are installed)"
                .into(),
        ));
    }
    Ok(bytes)
}
