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
    Network(String),

    #[error("no {kind} build for Minecraft {version}")]
    VersionNotFound { kind: &'static str, version: String },

    #[error("{file} is corrupt: expected {algorithm} {expected}, got {actual}")]
    ChecksumMismatch {
        file: String,
        algorithm: &'static str,
        expected: String,
        actual: String,
    },

    #[error("cancelled")]
    Cancelled,

    /// Carries the installer log so the UI can show what the installer said
    /// instead of a generic failure.
    #[error("the {installer} installer failed (exit {exit_code})")]
    InstallerFailed {
        installer: &'static str,
        exit_code: i32,
        log_path: String,
        log_tail: String,
    },

    #[error("no Java {required} or newer was found; install one or pin a JDK for this instance")]
    JavaNotFound { required: i64 },

    /// The port the instance wants is taken. Named separately from `Other`
    /// because the fix depends on *who* has it.
    #[error("port {port} is in use{}", taken_by.as_deref().map(|who| format!(" by \"{who}\"")).unwrap_or_default())]
    PortInUse { port: u16, taken_by: Option<String> },

    #[error("the disk holding {} is full", .path.display())]
    DiskFull { path: PathBuf },

    /// The machine could not reach the host at all: no DNS, no route, no
    /// response. Distinct from a server that answered with an error.
    #[error("{url} could not be reached: {detail}")]
    Offline { url: String, detail: String },

    #[error("{host} asked us to slow down; retry in {retry_after_s}s")]
    RateLimited { host: String, retry_after_s: u64 },

    /// Java is installed, but not a new enough one for this Minecraft version.
    #[error("Minecraft {mc_version} needs Java {required}; the newest found is Java {found}")]
    JavaTooOld {
        required: i64,
        found: i64,
        mc_version: String,
    },

    #[error("the Java pinned for \"{instance}\" is missing: {path}")]
    JavaPinnedMissing { instance: String, path: String },

    #[error("the Minecraft EULA has not been accepted for \"{0}\"")]
    EulaNotAccepted(String),

    #[error("\"{0}\" has no server installed yet")]
    NotInstalled(String),

    /// The folder exists but is not usable: a missing jar, an unreadable
    /// `instance.json`, a half-finished install.
    #[error("\"{name}\" is not in a usable state: {detail}")]
    InstanceCorrupt { name: String, detail: String },

    /// A zip or tar this app is reading is truncated, not the format it claims,
    /// or written by a newer build. The user's move is to pick another file.
    #[error("{label} could not be read as an archive: {detail}")]
    ArchiveUnreadable { label: String, detail: String },

    /// Something the user cannot have caused and cannot fix: a background task
    /// that died, a compressor that refused. It gets a plain apology and a
    /// pointer at the bug report, and keeps the technical text for whoever
    /// reads it.
    #[error("{context} failed: {detail}")]
    Internal {
        context: &'static str,
        detail: String,
    },

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
            AppError::Network(_) => "network",
            AppError::VersionNotFound { .. } => "version_not_found",
            AppError::ChecksumMismatch { .. } => "checksum_mismatch",
            AppError::Cancelled => "cancelled",
            AppError::InstallerFailed { .. } => "installer_failed",
            AppError::JavaNotFound { .. } => "java_not_found",
            AppError::PortInUse { .. } => "port_in_use",
            AppError::DiskFull { .. } => "disk_full",
            AppError::Offline { .. } => "offline",
            AppError::RateLimited { .. } => "rate_limited",
            AppError::JavaTooOld { .. } => "java_too_old",
            AppError::JavaPinnedMissing { .. } => "java_pinned_missing",
            AppError::EulaNotAccepted(_) => "eula_not_accepted",
            AppError::NotInstalled(_) => "not_installed",
            AppError::InstanceCorrupt { .. } => "instance_corrupt",
            AppError::ArchiveUnreadable { .. } => "archive_unreadable",
            AppError::Internal { .. } => "internal",
            AppError::Other(_) => "other",
        }
    }

    /// A background task or a library did something this app cannot recover
    /// from and the user did not cause.
    pub fn internal(context: &'static str, detail: impl std::fmt::Display) -> Self {
        AppError::Internal {
            context,
            detail: detail.to_string(),
        }
    }

    /// A zip or tar that cannot be read as one.
    pub fn archive(label: impl std::fmt::Display, detail: impl std::fmt::Display) -> Self {
        AppError::ArchiveUnreadable {
            label: label.to_string(),
            detail: detail.to_string(),
        }
    }

    pub fn io(action: &'static str, path: impl AsRef<Path>, source: std::io::Error) -> Self {
        // "No space left on device" is the one io error with a fix a person can
        // carry out, so it gets its own variant rather than a generic message.
        // Matched on the raw code because the named `ErrorKind` for it is newer
        // than this crate's minimum Rust version.
        if is_disk_full(&source) {
            return AppError::DiskFull {
                path: path.as_ref().to_path_buf(),
            };
        }
        AppError::Io {
            action,
            path: path.as_ref().to_path_buf(),
            source,
        }
    }

    /// What the app shows by default: one plain sentence, no Rust in it.
    ///
    /// The `Display` text is kept as well and travels as `technical`, behind a
    /// "details" expander — a stack of type names helps whoever files the bug
    /// report and nobody else.
    pub fn user_message(&self) -> String {
        match self {
            AppError::Db(_) | AppError::Migrate(_) => {
                "The app could not read its own database.".into()
            }
            AppError::Io { action, path, .. } => {
                format!("Could not {action} at {}.", path.display())
            }
            AppError::Json(_) => "A file this app relies on could not be read.".into(),
            AppError::InstanceNotFound(_) => "That server is no longer in the list.".into(),
            AppError::NameInUse(name) => format!("There is already a server called \"{name}\"."),
            AppError::PathInUse(path) => format!(
                "Another server already uses the folder {}.",
                path.display()
            ),
            AppError::InvalidName(_) => "That name cannot be used for a folder.".into(),
            AppError::FolderMissing { name, path } => format!(
                "The folder for \"{name}\" is not where the app left it ({}).",
                path.display()
            ),
            AppError::FolderNotEmpty(path) => {
                format!("The folder {} already has files in it.", path.display())
            }
            AppError::NotAServerFolder(path) => format!(
                "{} does not contain a Minecraft server.",
                path.display()
            ),
            AppError::InstanceRunning(name) => format!("\"{name}\" is running."),
            AppError::NotImplemented(what) => format!("This version of the app cannot {what} yet."),
            AppError::Network(_) => "The download could not be completed.".into(),
            AppError::VersionNotFound { kind, version } => {
                format!("There is no {kind} build for Minecraft {version}.")
            }
            AppError::ChecksumMismatch { file, .. } => format!(
                "The downloaded file {file} did not match the checksum published for it, so it was discarded."
            ),
            AppError::Cancelled => "Cancelled.".into(),
            AppError::InstallerFailed { installer, .. } => {
                format!("The {installer} installer did not finish.")
            }
            AppError::JavaNotFound { required } => {
                format!("Java {required} or newer is needed, and no Java was found on this computer.")
            }
            AppError::JavaTooOld {
                required,
                found,
                mc_version,
            } => format!(
                "Minecraft {mc_version} needs Java {required}, but the newest Java on this computer is Java {found}."
            ),
            AppError::JavaPinnedMissing { instance, path } => format!(
                "The Java chosen for \"{instance}\" is no longer at {path}."
            ),
            AppError::PortInUse {
                port,
                taken_by: Some(who),
            } => format!("Port {port} is already being used by \"{who}\"."),
            AppError::PortInUse {
                port,
                taken_by: None,
            } => format!("Port {port} is already being used by another program."),
            AppError::DiskFull { path } => format!(
                "The drive holding {} has no space left.",
                path.display()
            ),
            AppError::Offline { .. } => {
                "The app could not reach the internet.".into()
            }
            AppError::RateLimited { host, .. } => {
                format!("{host} is asking the app to slow down.")
            }
            AppError::EulaNotAccepted(name) => {
                format!("The Minecraft EULA has not been accepted for \"{name}\".")
            }
            AppError::NotInstalled(name) => format!("\"{name}\" has no server files yet."),
            AppError::InstanceCorrupt { name, .. } => {
                format!("\"{name}\" is missing files it needs to run.")
            }
            AppError::ArchiveUnreadable { label, .. } => {
                format!("{label} could not be opened.")
            }
            AppError::Internal { .. } => "Something inside the app went wrong.".into(),
            AppError::Other(message) => message.clone(),
        }
    }

    /// The next thing to do about it, when there is one. `None` means the
    /// message already says everything useful.
    pub fn hint(&self) -> Option<String> {
        let hint = match self {
            AppError::Db(_) | AppError::Migrate(_) => {
                "Close any other copy of the app that is running. If it keeps happening, use \
                 Report a problem so the log can be looked at."
            }
            AppError::Io { .. } => {
                "Check that the folder still exists and that another program does not have the \
                 file open."
            }
            AppError::Json(_) => "Use Report a problem to send the file along with the log.",
            AppError::NameInUse(_) => "Pick a different name.",
            AppError::PathInUse(_) | AppError::FolderNotEmpty(_) => "Choose an empty folder.",
            AppError::InvalidName(_) => {
                "Letters, numbers, spaces, dots, dashes and underscores work everywhere."
            }
            AppError::FolderMissing { .. } => {
                "If it was moved, use \"Locate folder…\" to point the app at it again."
            }
            AppError::NotAServerFolder(_) => {
                "Pick the folder that holds server.properties and the server jar."
            }
            AppError::InstanceRunning(_) => "Stop it first, then try again.",
            AppError::Network(_) => "Check the connection and try again.",
            AppError::VersionNotFound { .. } => "Pick a different version or a different server type.",
            AppError::ChecksumMismatch { .. } => {
                "Nothing was kept. Try the download again; if it fails twice, the file upstream is \
                 probably broken."
            }
            AppError::InstallerFailed { .. } => {
                "The installer's own output is in the details, and the full log is kept next to \
                 the instance."
            }
            AppError::JavaNotFound { .. } => {
                "Install a JDK (Temurin and Microsoft OpenJDK both work), then use Rescan in \
                 Settings. A specific Java can also be pinned per server."
            }
            AppError::JavaTooOld { required, .. } => {
                return Some(format!(
                    "Install Java {required} or newer, then use Rescan in Settings. Older Java \
                     stays installed; this app picks per server."
                ))
            }
            AppError::JavaPinnedMissing { .. } => {
                "Pick another Java for this server in its Settings tab, or clear the pin to let \
                 the app choose."
            }
            AppError::PortInUse {
                taken_by: Some(_), ..
            } => "Stop that server first, or give this one a different server-port in Config.",
            AppError::PortInUse { taken_by: None, .. } => {
                "Close the other program, or change server-port in the Config tab."
            }
            AppError::DiskFull { .. } => {
                "Free some space, or move this server to another drive, and try again."
            }
            AppError::Offline { .. } => {
                "Check the network. A VPN, proxy or firewall blocking the app will look the same \
                 as being offline."
            }
            AppError::RateLimited { retry_after_s, .. } => {
                return Some(format!(
                    "Wait about {retry_after_s} seconds and try again. Nothing was lost."
                ))
            }
            AppError::EulaNotAccepted(_) => {
                "Open the server's Settings tab and accept the Minecraft EULA there. The app never \
                 accepts it for you."
            }
            AppError::NotInstalled(_) => "Use Install in the server's Settings tab first.",
            AppError::InstanceCorrupt { .. } => {
                "Reinstalling the server files keeps worlds, configuration and mods. Restoring a \
                 backup is the other way back."
            }
            AppError::ArchiveUnreadable { .. } => {
                "The file may be incomplete, or not the kind of archive it looks like. Try \
                 another backup or download it again."
            }
            AppError::Internal { .. } => {
                "Nothing was half-written. If it happens again, use Report a problem so the log \
                 comes with it."
            }
            AppError::Cancelled
            | AppError::InstanceNotFound(_)
            | AppError::NotImplemented(_)
            | AppError::Other(_) => return None,
        };
        Some(hint.to_string())
    }
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // `message` is the plain-language one because it is what the UI shows by
        // default; the Display text travels as `technical` for the expander.
        let mut s = serializer.serialize_struct("AppError", 5)?;
        s.serialize_field("kind", self.kind())?;
        s.serialize_field("message", &self.user_message())?;
        s.serialize_field("hint", &self.hint())?;
        s.serialize_field("technical", &self.to_string())?;
        match self {
            AppError::InstallerFailed {
                log_path, log_tail, ..
            } => s.serialize_field(
                "detail",
                &serde_json::json!({ "logPath": log_path, "logTail": log_tail }),
            )?,
            AppError::JavaNotFound { required } => {
                s.serialize_field("detail", &serde_json::json!({ "required": required }))?
            }
            AppError::JavaTooOld {
                required,
                found,
                mc_version,
            } => s.serialize_field(
                "detail",
                &serde_json::json!({ "required": required, "found": found, "mcVersion": mc_version }),
            )?,
            AppError::PortInUse { port, taken_by } => s.serialize_field(
                "detail",
                &serde_json::json!({ "port": port, "takenBy": taken_by }),
            )?,
            AppError::RateLimited {
                host,
                retry_after_s,
            } => s.serialize_field(
                "detail",
                &serde_json::json!({ "host": host, "retryAfterSeconds": retry_after_s }),
            )?,
            _ => s.serialize_field("detail", &serde_json::Value::Null)?,
        }
        s.end()
    }
}
/// ENOSPC on Unix; ERROR_DISK_FULL / ERROR_HANDLE_DISK_FULL on Windows.
fn is_disk_full(source: &std::io::Error) -> bool {
    match source.raw_os_error() {
        Some(code) if cfg!(windows) => code == 39 || code == 112,
        Some(code) => code == 28,
        None => false,
    }
}

/// Turns a reqwest failure into either "the internet is not reachable" or a
/// plain transfer error, because only the first has an action attached to it.
pub fn from_reqwest(url: &str, source: &reqwest::Error) -> AppError {
    if source.is_connect() || source.is_timeout() || source.is_request() {
        return AppError::Offline {
            url: url.to_string(),
            detail: source.to_string(),
        };
    }
    AppError::Network(format!("{url} could not be reached: {source}"))
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
        assert_eq!(json["message"], "There is already a server called \"survival\".");
        assert_eq!(
            json["technical"],
            "an instance named \"survival\" already exists"
        );
    }
    /// Every variant, so the checks below cover the whole surface rather than
    /// whichever ones somebody remembered.
    fn one_of_each() -> Vec<AppError> {
        vec![
            AppError::Db(sqlx::Error::RowNotFound),
            AppError::Io {
                action: "create folder",
                path: PathBuf::from("/srv/mc"),
                source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
            },
            AppError::Json(serde_json::from_str::<i32>("nope").unwrap_err()),
            AppError::InstanceNotFound("7".into()),
            AppError::NameInUse("Survival".into()),
            AppError::PathInUse(PathBuf::from("/srv/mc")),
            AppError::InvalidName("con".into()),
            AppError::FolderMissing {
                name: "Survival".into(),
                path: PathBuf::from("/srv/mc"),
            },
            AppError::FolderNotEmpty(PathBuf::from("/srv/mc")),
            AppError::NotAServerFolder(PathBuf::from("/srv/mc")),
            AppError::InstanceRunning("Survival".into()),
            AppError::NotImplemented("import a CurseForge pack"),
            AppError::Network("paper could not be reached".into()),
            AppError::VersionNotFound {
                kind: "Paper",
                version: "26.2".into(),
            },
            AppError::ChecksumMismatch {
                file: "paper.jar".into(),
                algorithm: "SHA-256",
                expected: "aa".into(),
                actual: "bb".into(),
            },
            AppError::Cancelled,
            AppError::InstallerFailed {
                installer: "Forge",
                exit_code: 1,
                log_path: "/srv/mc/.msm/installer-forge.log".into(),
                log_tail: "boom".into(),
            },
            AppError::JavaNotFound { required: 21 },
            AppError::JavaTooOld {
                required: 21,
                found: 17,
                mc_version: "1.21.4".into(),
            },
            AppError::JavaPinnedMissing {
                instance: "Survival".into(),
                path: "C:/jdk17/bin/java.exe".into(),
            },
            AppError::PortInUse {
                port: 25565,
                taken_by: Some("Creative".into()),
            },
            AppError::PortInUse {
                port: 25565,
                taken_by: None,
            },
            AppError::DiskFull {
                path: PathBuf::from("/srv/mc"),
            },
            AppError::Offline {
                url: "https://api.papermc.io".into(),
                detail: "dns error".into(),
            },
            AppError::RateLimited {
                host: "api.modrinth.com".into(),
                retry_after_s: 30,
            },
            AppError::EulaNotAccepted("Survival".into()),
            AppError::NotInstalled("Survival".into()),
            AppError::InstanceCorrupt {
                name: "Survival".into(),
                detail: "server.jar is missing".into(),
            },
            AppError::ArchiveUnreadable {
                label: "survival-20260819.tar.zst".into(),
                detail: "unexpected end of file".into(),
            },
            AppError::Internal {
                context: "writing the archive",
                detail: "the task panicked".into(),
            },
            AppError::Other("something else".into()),
        ]
    }

    #[test]
    fn every_user_message_reads_like_a_sentence_not_like_a_stack_trace() {
        for err in one_of_each() {
            let message = err.user_message();
            assert!(!message.is_empty(), "{:?} has no user message", err.kind());
            // `Other` carries a sentence its call site already wrote for a
            // person, so only the shaped variants are checked for punctuation.
            if err.kind() != "other" {
                assert!(
                    message.ends_with('.') || message.ends_with('?'),
                    "{}: {message:?} should be a sentence",
                    err.kind()
                );
            }

            // Words that mean nothing to somebody who did not write the app.
            for noise in [
                "sqlx", "reqwest", "serde", "Error {", "os error", "Utf8", "None", "Some(",
                "unwrap", "::", "panicked",
            ] {
                assert!(
                    !message.contains(noise),
                    "{} leaks \"{noise}\": {message}",
                    err.kind()
                );
            }
        }
    }

    #[test]
    fn the_errors_a_user_can_do_something_about_say_what() {
        // The list the polish pass called out by name.
        let actionable = [
            AppError::JavaNotFound { required: 21 },
            AppError::JavaTooOld {
                required: 21,
                found: 17,
                mc_version: "1.21.4".into(),
            },
            AppError::PortInUse {
                port: 25565,
                taken_by: None,
            },
            AppError::DiskFull {
                path: PathBuf::from("/srv/mc"),
            },
            AppError::Offline {
                url: "https://api.modrinth.com".into(),
                detail: "dns error".into(),
            },
            AppError::RateLimited {
                host: "api.modrinth.com".into(),
                retry_after_s: 30,
            },
            AppError::InstanceCorrupt {
                name: "Survival".into(),
                detail: "server.jar is missing".into(),
            },
            AppError::EulaNotAccepted("Survival".into()),
        ];

        for err in actionable {
            let hint = err.hint().unwrap_or_else(|| panic!("{} has no hint", err.kind()));
            assert!(hint.len() > 20, "{}: {hint:?} is not advice", err.kind());
        }
    }

    #[test]
    fn cancelling_is_not_an_error_to_apologise_for() {
        assert_eq!(AppError::Cancelled.user_message(), "Cancelled.");
        assert!(AppError::Cancelled.hint().is_none());
    }

    #[test]
    fn the_technical_text_travels_alongside_the_readable_one() {
        let err = AppError::JavaTooOld {
            required: 21,
            found: 17,
            mc_version: "1.21.4".into(),
        };
        let json = serde_json::to_value(&err).expect("serialize");

        assert_eq!(json["kind"], "java_too_old");
        assert_eq!(
            json["message"],
            "Minecraft 1.21.4 needs Java 21, but the newest Java on this computer is Java 17."
        );
        assert!(json["hint"].as_str().unwrap().contains("Rescan"));
        assert_eq!(
            json["technical"],
            "Minecraft 1.21.4 needs Java 21; the newest found is Java 17"
        );
        assert_eq!(json["detail"]["found"], 17);
    }

    #[test]
    fn a_full_disk_is_recognised_from_the_os_code() {
        let code = if cfg!(windows) { 112 } else { 28 };
        let err = AppError::io(
            "write the archive",
            Path::new("/srv/mc/backup.tar.zst"),
            std::io::Error::from_raw_os_error(code),
        );

        assert_eq!(err.kind(), "disk_full");
        assert!(err.user_message().contains("no space left"));

        // Anything else stays a plain io error.
        let other = AppError::io(
            "write the archive",
            Path::new("/srv/mc"),
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
        );
        assert_eq!(other.kind(), "io");
    }

    #[test]
    fn every_kind_is_unique_so_the_ui_can_branch_on_it() {
        let mut seen = std::collections::HashSet::new();
        for err in one_of_each() {
            // PortInUse appears twice on purpose; both share one kind.
            if err.kind() == "port_in_use" {
                continue;
            }
            assert!(seen.insert(err.kind()), "duplicate kind {}", err.kind());
        }
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
