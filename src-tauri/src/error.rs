use std::path::{Path, PathBuf};

use serde::ser::{Serialize, SerializeStruct, Serializer};

/// Every fallible operation in the backend returns this. Command handlers never
/// panic: they map failures onto a variant here, which the UI renders as a
/// readable message plus a stable `kind` it can branch on.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),

    #[error("database migration failed: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("{action} failed for {}: {source}", path.display())]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("could not read JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("no instance with id {0}")]
    InstanceNotFound(String),

    #[error("an instance named \"{0}\" already exists")]
    NameInUse(String),

    #[error("another instance already uses the folder {}", .0.display())]
    PathInUse(PathBuf),

    #[error("invalid instance name: {0}")]
    InvalidName(String),

    /// Recoverable: the UI offers "Locate folder…" rather than treating this as a failure.
    #[error("the folder for \"{name}\" is missing: {}", path.display())]
    FolderMissing { name: String, path: PathBuf },

    #[error("the folder {} is not empty", .0.display())]
    FolderNotEmpty(PathBuf),

    #[error("{} does not look like a Minecraft server folder", .0.display())]
    NotAServerFolder(PathBuf),

    #[error("\"{0}\" is running; stop it first")]
    InstanceRunning(String),

    #[error("this build cannot do that yet: {0}")]
    NotImplemented(&'static str),

    #[error("{0}")]
    Other(String),
}

impl AppError {
    /// Stable machine-readable discriminator for the frontend.
    pub fn kind(&self) -> &'static str {
        match self {
            AppError::Db(_) | AppError::Migrate(_) => "database",
            AppError::Io { .. } => "io",
            AppError::Json(_) => "json",
            AppError::InstanceNotFound(_) => "instance_not_found",
            AppError::NameInUse(_) => "name_in_use",
            AppError::PathInUse(_) => "path_in_use",
            AppError::InvalidName(_) => "invalid_name",
            AppError::FolderMissing { .. } => "folder_missing",
            AppError::FolderNotEmpty(_) => "folder_not_empty",
            AppError::NotAServerFolder(_) => "not_a_server_folder",
            AppError::InstanceRunning(_) => "instance_running",
            AppError::NotImplemented(_) => "not_implemented",
            AppError::Other(_) => "other",
        }
    }

    pub fn io(action: &'static str, path: impl AsRef<Path>, source: std::io::Error) -> Self {
        AppError::Io {
            action,
            path: path.as_ref().to_path_buf(),
            source,
        }
    }
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("AppError", 2)?;
        s.serialize_field("kind", self.kind())?;
        s.serialize_field("message", &self.to_string())?;
        s.end()
    }
}

pub type AppResult<T> = Result<T, AppError>;

/// `io::Result` -> `AppResult` with the path and the attempted action attached.
pub trait IoContext<T> {
    fn ctx(self, action: &'static str, path: impl AsRef<Path>) -> AppResult<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn ctx(self, action: &'static str, path: impl AsRef<Path>) -> AppResult<T> {
        self.map_err(|source| AppError::io(action, path, source))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_kind_and_message() {
        let err = AppError::NameInUse("survival".into());
        let json = serde_json::to_value(&err).expect("serialize");
        assert_eq!(json["kind"], "name_in_use");
        assert_eq!(json["message"], "an instance named \"survival\" already exists");
    }

    #[test]
    fn io_errors_carry_the_path() {
        let err = AppError::io(
            "create folder",
            Path::new("/tmp/x"),
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
        );
        assert_eq!(err.kind(), "io");
        assert!(err.to_string().contains("create folder"));
        assert!(err.to_string().contains("denied"));
    }
}
