//! Read session files from disk, shelling out to `charles convert` for `.chls`.

use std::path::Path;

use tokio::process::Command;

use super::{Session, SessionSource, sniff};
use crate::config::Config;
use crate::error::CharlesError;

/// Parse a `.chls` / `.har` / `.chlsj` file into a [`Session`].
pub async fn read_session_file(cfg: &Config, path: &Path) -> Result<Session, CharlesError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let bytes = if ext == "chls" {
        convert_to_chlsj(cfg, path).await?
    } else {
        tokio::fs::read(path).await?
    };

    let transactions = sniff::parse_bytes(bytes)?;
    Ok(Session {
        source: SessionSource::File(path.to_path_buf()),
        transactions,
    })
}

/// Convert in-memory session bytes (e.g. a downloaded native `.chls`) to
/// `.chlsj` bytes by writing them to a temp file and shelling out to Charles.
pub async fn convert_bytes(
    cfg: &Config,
    bytes: &[u8],
    in_ext: &str,
) -> Result<Vec<u8>, CharlesError> {
    if !cfg.charles_bin.exists() {
        return Err(CharlesError::CharlesBinMissing(cfg.charles_bin.clone()));
    }
    let infile = tempfile::Builder::new()
        .suffix(&format!(".{in_ext}"))
        .tempfile()?;
    tokio::fs::write(infile.path(), bytes).await?;
    convert_to_chlsj(cfg, infile.path()).await
}

/// Convert a native `.chls` file to `.chlsj` bytes via the Charles binary.
pub async fn convert_to_chlsj(cfg: &Config, infile: &Path) -> Result<Vec<u8>, CharlesError> {
    if !cfg.charles_bin.exists() {
        return Err(CharlesError::CharlesBinMissing(cfg.charles_bin.clone()));
    }
    let out = tempfile::Builder::new().suffix(".chlsj").tempfile()?;
    let out_path = out.path().to_path_buf();

    let result = Command::new(&cfg.charles_bin)
        .arg("convert")
        .arg(infile)
        .arg(&out_path)
        .output()
        .await?;

    if !result.status.success() {
        return Err(CharlesError::ConvertFailed(
            String::from_utf8_lossy(&result.stderr).trim().to_string(),
        ));
    }
    Ok(tokio::fs::read(&out_path).await?)
}
