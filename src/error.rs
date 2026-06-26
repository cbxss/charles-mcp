//! Error type shared across the crate, with a mapping into the MCP wire error.

use std::path::PathBuf;

use rmcp::ErrorData;

/// Everything that can go wrong talking to Charles or parsing a session.
#[derive(Debug, thiserror::Error)]
pub enum CharlesError {
    #[error(
        "cannot reach the Charles Web Interface at {proxy}: {source}; is Charles running with \
         Proxy → Web Interface enabled, and are --proxy-host/--proxy-port correct?"
    )]
    Unreachable {
        proxy: String,
        #[source]
        source: reqwest::Error,
    },

    #[error(
        "the Charles Web Interface requires authentication (set --web-user/--web-pass) or the credentials are wrong"
    )]
    Unauthorized,

    #[error(
        "Charles returned HTTP {status} for `{path}`; verify the Web Interface is enabled and this \
         action is supported in your Charles version"
    )]
    HttpStatus { status: u16, path: String },

    #[error(
        "could not locate the `{0}` capability in the Charles Web Interface; \
         enable it under Proxy → Web Interface Settings, or the endpoint layout changed"
    )]
    EndpointNotFound(&'static str),

    #[error("the Charles binary was not found at {0} (set --charles-bin / CHARLES_BIN)")]
    CharlesBinMissing(PathBuf),

    #[error(
        "`charles convert` failed: {0}; ensure the input is a valid .chls and the Charles app can \
         launch (set --charles-bin if it is not at the default path)"
    )]
    ConvertFailed(String),

    #[error("unrecognized session format; supported inputs are .chls, .har, and .chlsj")]
    UnknownFormat,

    #[error("failed to parse session: {0}; the file may be truncated or not a Charles/HAR session")]
    Parse(String),

    #[error("invalid argument: {0}")]
    InvalidArg(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl From<CharlesError> for ErrorData {
    fn from(e: CharlesError) -> Self {
        use CharlesError::*;
        match e {
            // Things the caller can fix by changing input/config/connection.
            Unreachable { .. }
            | Unauthorized
            | HttpStatus { .. }
            | EndpointNotFound(_)
            | CharlesBinMissing(_)
            | InvalidArg(_)
            | UnknownFormat => ErrorData::invalid_request(e.to_string(), None),
            // Internal/unexpected failures.
            ConvertFailed(_) | Parse(_) | Io(_) | Http(_) | Json(_) => {
                ErrorData::internal_error(e.to_string(), None)
            }
        }
    }
}
