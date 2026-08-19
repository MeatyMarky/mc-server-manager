//! Port conflict detection, run before a server is started.
//!
//! "Failed to bind to port" arrives 20 seconds into startup and tells the user
//! nothing about *which* of their servers already holds it. Checking first, and
//! naming the other instance when it is one of ours, turns that into a sentence
//! someone can act on.

use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::path::Path;

use sqlx::SqlitePool;

use crate::error::{AppError, AppResult};

pub const DEFAULT_PORT: u16 = 25565;

/// Reads one key out of `server.properties` without disturbing the file.
///
/// The full editor (comments, ordering, unknown keys) lands in phase 4; this is
/// a read-only peek that has to cope with the same file shape.
pub fn read_property(contents: &str, key: &str) -> Option<String> {
    contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .filter_map(|line| line.split_once('='))
        .find(|(name, _)| name.trim() == key)
        .map(|(_, value)| value.trim().to_string())
}

/// The port an instance will bind, defaulting to 25565 like the server does.
pub fn configured_port(instance_path: &Path) -> u16 {
    let path = crate::paths::server_properties_path(instance_path);
    std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| read_property(&contents, "server-port"))
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|port| *port != 0)
        .unwrap_or(DEFAULT_PORT)
}

/// Whether anything is currently listening on the port.
///
/// Both the wildcard address and loopback are probed: on Windows a socket bound
/// to `127.0.0.1` does not stop a later bind to `0.0.0.0`, so checking only the
/// wildcard reports a taken port as free.
pub fn port_is_free(port: u16) -> bool {
    let wildcard = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port)).is_ok();
    let loopback = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)).is_ok();
    wildcard && loopback
}

/// What a pre-start check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortCheck {
    Free,
    /// Taken by another instance this app manages.
    TakenByInstance { port: u16, instance: String },
    /// Taken by something else on the machine.
    TakenByOther { port: u16 },
}

impl PortCheck {
    /// The conflict as an error, or `None` when the port is free.
    ///
    /// A typed variant rather than a sentence: the UI wants to offer "open the
    /// Config tab" for one case and "stop that server" for the other, and it
    /// cannot branch on prose.
    pub fn as_error(&self) -> Option<AppError> {
        match self {
            PortCheck::Free => None,
            PortCheck::TakenByInstance { port, instance } => Some(AppError::PortInUse {
                port: *port,
                taken_by: Some(instance.clone()),
            }),
            PortCheck::TakenByOther { port } => Some(AppError::PortInUse {
                port: *port,
                taken_by: None,
            }),
        }
    }

    /// The same thing as one line, for the pre-start banner.
    pub fn message(&self) -> Option<String> {
        self.as_error().map(|err| {
            let hint = err.hint().unwrap_or_default();
            format!("{} {hint}", err.user_message()).trim_end().to_string()
        })
    }
}

/// Decides the outcome from the two facts a caller can gather. Pure, so the
/// message shape is testable without opening sockets.
pub fn classify(port: u16, free: bool, owner: Option<String>) -> PortCheck {
    match (free, owner) {
        (true, None) => PortCheck::Free,
        // A live instance of ours owns it even if the socket looks bindable
        // (it may not have bound yet, or it binds a specific interface).
        (_, Some(instance)) => PortCheck::TakenByInstance { port, instance },
        (false, None) => PortCheck::TakenByOther { port },
    }
}

/// Full check: which port this instance wants, whether it is free, and whether
/// another *running* instance of ours is configured for it.
pub async fn check(
    pool: &SqlitePool,
    instance_id: i64,
    instance_path: &Path,
    live_uuids: &[String],
) -> AppResult<PortCheck> {
    let port = configured_port(instance_path);

    let others: Vec<(String, String)> =
        sqlx::query_as("SELECT uuid, path FROM instances WHERE id != ?")
            .bind(instance_id)
            .fetch_all(pool)
            .await?;

    let mut owner = None;
    for (uuid, path) in others {
        if !live_uuids.contains(&uuid) {
            continue;
        }
        if configured_port(Path::new(&path)) == port {
            owner = sqlx::query_scalar::<_, String>("SELECT name FROM instances WHERE uuid = ?")
                .bind(&uuid)
                .fetch_optional(pool)
                .await?;
            break;
        }
    }

    Ok(classify(port, port_is_free(port), owner))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_port_out_of_server_properties() {
        let properties = "#Minecraft server properties\n\
                          #Mon Aug 18 12:00:00 UTC 2026\n\
                          motd=A Minecraft Server\n\
                          server-port=25577\n\
                          max-players=20\n";
        assert_eq!(read_property(properties, "server-port").as_deref(), Some("25577"));
        assert_eq!(read_property(properties, "max-players").as_deref(), Some("20"));
        assert_eq!(read_property(properties, "missing"), None);
    }

    #[test]
    fn commented_out_keys_are_not_values() {
        let properties = "#server-port=25599\nserver-port=25565\n";
        assert_eq!(read_property(properties, "server-port").as_deref(), Some("25565"));
    }

    #[test]
    fn a_missing_or_broken_file_falls_back_to_the_default_port() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(configured_port(dir.path()), DEFAULT_PORT);

        std::fs::write(
            crate::paths::server_properties_path(dir.path()),
            b"server-port=not-a-number\n",
        )
        .unwrap();
        assert_eq!(configured_port(dir.path()), DEFAULT_PORT);

        std::fs::write(
            crate::paths::server_properties_path(dir.path()),
            b"server-port=0\n",
        )
        .unwrap();
        assert_eq!(configured_port(dir.path()), DEFAULT_PORT, "0 means default");
    }

    #[test]
    fn a_configured_port_is_used() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            crate::paths::server_properties_path(dir.path()),
            b"server-port=25580\n",
        )
        .unwrap();
        assert_eq!(configured_port(dir.path()), 25580);
    }

    #[test]
    fn conflicts_name_our_own_instance_when_we_know_it() {
        let ours = classify(25565, false, Some("Survival".into()));
        let message = ours.message().unwrap();
        assert!(message.contains("25565"), "{message}");
        assert!(message.contains("Survival"), "{message}");
        // The fix differs per case, which is why this is a typed error and not
        // one sentence with a blank in it.
        assert!(message.contains("Stop that server"), "{message}");
        assert_eq!(
            ours.as_error().unwrap().kind(),
            "port_in_use",
            "the UI branches on the kind"
        );

        let theirs = classify(25565, false, None);
        let message = theirs.message().unwrap();
        assert!(message.contains("another program"), "{message}");
        assert!(message.contains("server-port"), "{message}");

        assert_eq!(classify(25565, true, None), PortCheck::Free);
        assert!(classify(25565, true, None).message().is_none());
        assert!(classify(25565, true, None).as_error().is_none());
    }

    #[test]
    fn a_live_instance_of_ours_wins_over_a_bindable_socket() {
        // The other server may not have bound yet; we still know it owns the port.
        assert_eq!(
            classify(25565, true, Some("Creative".into())),
            PortCheck::TakenByInstance {
                port: 25565,
                instance: "Creative".into()
            }
        );
    }

    #[test]
    fn a_bound_port_is_detected_on_either_interface() {
        // Loopback only: Windows still allows a wildcard bind, which is exactly
        // why port_is_free probes both.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(!port_is_free(port), "a loopback listener means the port is taken");
        drop(listener);

        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(!port_is_free(port), "a wildcard listener means the port is taken");
        drop(listener);
    }

    #[tokio::test]
    async fn the_check_names_the_other_running_instance() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mine = dir.path().join("mine");
        let theirs = dir.path().join("theirs");
        std::fs::create_dir_all(&mine).unwrap();
        std::fs::create_dir_all(&theirs).unwrap();
        for path in [&mine, &theirs] {
            std::fs::write(
                crate::paths::server_properties_path(path),
                b"server-port=25599\n",
            )
            .unwrap();
        }

        let now = crate::db::now_rfc3339();
        for (uuid, name, path) in [
            ("u1", "Mine", mine.to_string_lossy().to_string()),
            ("u2", "Theirs", theirs.to_string_lossy().to_string()),
        ] {
            sqlx::query(
                "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                    jvm_args, server_args, created_at, updated_at)
                 VALUES (?, ?, ?, 'paper', '1.21.4', 'jar', '[]', '[]', ?, ?)",
            )
            .bind(uuid)
            .bind(name)
            .bind(path)
            .bind(&now)
            .bind(&now)
            .execute(&pool)
            .await
            .unwrap();
        }

        // "Theirs" is running and configured for the same port.
        let taken = super::check(&pool, 1, &mine, &["u2".to_string()])
            .await
            .unwrap();
        assert_eq!(
            taken,
            PortCheck::TakenByInstance {
                port: 25599,
                instance: "Theirs".into()
            }
        );

        // With nothing of ours running, a free port is free.
        let free = super::check(&pool, 1, &mine, &[]).await.unwrap();
        assert_eq!(free, PortCheck::Free);
    }
}
