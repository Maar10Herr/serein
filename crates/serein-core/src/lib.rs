pub mod inference;
pub mod install;
pub mod model;
pub mod policy;
pub mod protocol;
pub mod storage;
pub use protocol::*;
use std::path::PathBuf;
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug)]
pub struct Error(pub &'static str, pub String);
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.0, self.1)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        Self("ACCESS_DENIED", "Check local file permissions.".into())
    }
}
impl From<serde_json::Error> for Error {
    fn from(_: serde_json::Error) -> Self {
        Self(
            "INVALID_REQUEST",
            "Invalid JSON or unsupported fields.".into(),
        )
    }
}
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        if matches!(&e,rusqlite::Error::SqliteFailure(x,_) if x.code==rusqlite::ErrorCode::DatabaseBusy || x.code==rusqlite::ErrorCode::DatabaseLocked)
        {
            Self("DB_BUSY", "Try again shortly.".into())
        } else {
            Self(
                "STORAGE_FULL",
                format!(
                    "Local database operation failed: {}",
                    e.sqlite_error_code()
                        .map(|x| format!("{x:?}"))
                        .unwrap_or_default()
                ),
            )
        }
    }
}
pub fn invalid(s: &str) -> Error {
    Error("INVALID_REQUEST", s.into())
}
pub fn error_value(e: &Error) -> serde_json::Value {
    serde_json::json!({"protocol":1,"status":"error","error":{"code":e.0,"retryable":e.0=="DB_BUSY","remedy":e.1}})
}
pub fn root() -> PathBuf {
    if let Some(p) = std::env::var_os("SEREIN_DATA_DIR") {
        return PathBuf::from(p);
    }
    let h = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    if cfg!(target_os = "macos") {
        h.join("Library/Application Support/Serein")
    } else if cfg!(windows) {
        PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("Serein")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or(h.join(".local/share"))
            .join("serein")
    }
}
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
pub fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
pub fn check_id(s: &str) -> Result<()> {
    uuid::Uuid::parse_str(s)
        .map(|_| ())
        .map_err(|_| invalid("Expected a UUID identifier."))
}
pub fn hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
