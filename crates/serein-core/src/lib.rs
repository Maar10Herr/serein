pub mod algorithm;
pub mod importance;
pub mod inference;
pub mod install;
pub mod model;
pub mod policy;
pub mod protocol;
pub mod retrieval;
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
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::PermissionDenied => {
                Self("ACCESS_DENIED", "Check local file permissions.".into())
            }
            std::io::ErrorKind::NotFound => {
                Self("NOT_FOUND", "A required local file was not found.".into())
            }
            _ => Self("IO_ERROR", "A local file operation failed.".into()),
        }
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
        use rusqlite::ErrorCode;

        match e.sqlite_error_code() {
            Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => {
                Self("DB_BUSY", "Try again shortly.".into())
            }
            Some(ErrorCode::DiskFull) => Self(
                "STORAGE_FULL",
                "Free up local disk space, then retry.".into(),
            ),
            Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase) => Self(
                "DB_CORRUPT",
                "The local database is damaged or invalid. Restore a backup or contact support."
                    .into(),
            ),
            Some(ErrorCode::PermissionDenied | ErrorCode::ReadOnly) => {
                Self("ACCESS_DENIED", "Check local file permissions.".into())
            }
            Some(ErrorCode::CannotOpen | ErrorCode::SystemIoFailure) => {
                Self("IO_ERROR", "A local database file operation failed.".into())
            }
            // SQLITE_NOTFOUND describes a missing SQLite file-control operation in some
            // contexts, not necessarily a missing filesystem path. Keep it generic.
            _ => Self("DB_ERROR", "A local database operation failed.".into()),
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

#[cfg(test)]
mod error_mapping_tests {
    use super::Error;
    use rusqlite::{ffi, ErrorCode};

    fn sqlite_error(code: ErrorCode) -> rusqlite::Error {
        rusqlite::Error::SqliteFailure(
            ffi::Error {
                code,
                extended_code: code as i32,
            },
            None,
        )
    }

    #[test]
    fn sqlite_busy_and_locked_are_retryable_busy_errors() {
        for code in [ErrorCode::DatabaseBusy, ErrorCode::DatabaseLocked] {
            let error = Error::from(sqlite_error(code));
            assert_eq!(error.0, "DB_BUSY");
            assert_eq!(error.1, "Try again shortly.");
        }
    }

    #[test]
    fn sqlite_storage_and_corruption_codes_are_distinct() {
        let full = Error::from(sqlite_error(ErrorCode::DiskFull));
        assert_eq!(full.0, "STORAGE_FULL");

        for code in [ErrorCode::DatabaseCorrupt, ErrorCode::NotADatabase] {
            let corrupt = Error::from(sqlite_error(code));
            assert_eq!(corrupt.0, "DB_CORRUPT");
        }
    }

    #[test]
    fn sqlite_access_and_io_errors_have_their_own_codes() {
        for code in [ErrorCode::PermissionDenied, ErrorCode::ReadOnly] {
            assert_eq!(Error::from(sqlite_error(code)).0, "ACCESS_DENIED");
        }

        for code in [ErrorCode::CannotOpen, ErrorCode::SystemIoFailure] {
            assert_eq!(Error::from(sqlite_error(code)).0, "IO_ERROR");
        }
    }

    #[test]
    fn other_sqlite_errors_stay_generic_and_do_not_expose_details() {
        let error = Error::from(sqlite_error(ErrorCode::ConstraintViolation));
        assert_eq!(error.0, "DB_ERROR");
        assert_eq!(error.1, "A local database operation failed.");
    }

    #[test]
    fn io_errors_distinguish_permission_not_found_and_other_failures() {
        let denied = Error::from(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        assert_eq!(denied.0, "ACCESS_DENIED");

        let missing = Error::from(std::io::Error::from(std::io::ErrorKind::NotFound));
        assert_eq!(missing.0, "NOT_FOUND");

        let other = Error::from(std::io::Error::from(std::io::ErrorKind::Other));
        assert_eq!(other.0, "IO_ERROR");

        let messages = [denied.1, missing.1, other.1].join(" ");
        assert!(!messages.contains('/') && !messages.contains('\\'));
    }
}
