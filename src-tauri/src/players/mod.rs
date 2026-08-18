//! Ops, whitelist and bans.
//!
//! A running server holds these lists in memory and rewrites the JSON files on
//! shutdown, so editing a file under a live server is silently undone. Every
//! mutation therefore goes through [`mutate`], the single gate:
//!
//!   * running → send the server's own command on stdin, then re-read the file
//!   * stopped → atomic temp-file write
//!
//! No other code in this app touches `ops.json`, `whitelist.json`,
//! `banned-players.json` or `banned-ips.json`.

pub mod files;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::db::record_event;
use crate::error::{AppError, AppResult};
use crate::instance;
use crate::process::supervisor;
use crate::state::AppState;

pub use files::{BannedIp, BannedPlayer, ListKind, OpEntry, WhitelistEntry};

/// Everything the players view shows, read from the server's own files.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct PlayerLists {
    pub ops: Vec<OpEntry>,
    pub whitelist: Vec<WhitelistEntry>,
    pub banned_players: Vec<BannedPlayer>,
    pub banned_ips: Vec<BannedIp>,
    /// Players seen in the console, newest first.
    pub seen: Vec<SeenPlayer>,
    /// Whether the whitelist is enforced at all (`white-list` in properties).
    pub whitelist_enabled: bool,
    /// Live instances take stdin commands; stopped ones are edited on disk.
    pub running: bool,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct SeenPlayer {
    pub uuid: String,
    pub name: String,
    pub first_seen: String,
    pub last_seen: String,
}

/// One change to one list.
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(tag = "action", rename_all = "snake_case")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub enum Mutation {
    Op {
        player: String,
        /// 1–4; the server only stores it, `/op` always grants the configured level.
        #[ts(type = "number | null")]
        level: Option<i64>,
    },
    Deop {
        player: String,
    },
    WhitelistAdd {
        player: String,
    },
    WhitelistRemove {
        player: String,
    },
    Ban {
        player: String,
        reason: Option<String>,
    },
    Pardon {
        player: String,
    },
    BanIp {
        ip: String,
        reason: Option<String>,
    },
    PardonIp {
        ip: String,
    },
}

impl Mutation {
    /// The command a running server understands, exactly as typed in its console.
    pub fn console_command(&self) -> String {
        match self {
            Mutation::Op { player, .. } => format!("op {player}"),
            Mutation::Deop { player } => format!("deop {player}"),
            Mutation::WhitelistAdd { player } => format!("whitelist add {player}"),
            Mutation::WhitelistRemove { player } => format!("whitelist remove {player}"),
            Mutation::Ban { player, reason } => match reason.as_deref().map(str::trim) {
                Some(reason) if !reason.is_empty() => format!("ban {player} {reason}"),
                _ => format!("ban {player}"),
            },
            Mutation::Pardon { player } => format!("pardon {player}"),
            Mutation::BanIp { ip, reason } => match reason.as_deref().map(str::trim) {
                Some(reason) if !reason.is_empty() => format!("ban-ip {ip} {reason}"),
                _ => format!("ban-ip {ip}"),
            },
            Mutation::PardonIp { ip } => format!("pardon-ip {ip}"),
        }
    }

    pub fn list(&self) -> ListKind {
        match self {
            Mutation::Op { .. } | Mutation::Deop { .. } => ListKind::Ops,
            Mutation::WhitelistAdd { .. } | Mutation::WhitelistRemove { .. } => ListKind::Whitelist,
            Mutation::Ban { .. } | Mutation::Pardon { .. } => ListKind::BannedPlayers,
            Mutation::BanIp { .. } | Mutation::PardonIp { .. } => ListKind::BannedIps,
        }
    }

    /// The subject, for events and error messages.
    pub fn subject(&self) -> &str {
        match self {
            Mutation::Op { player, .. }
            | Mutation::Deop { player }
            | Mutation::WhitelistAdd { player }
            | Mutation::WhitelistRemove { player }
            | Mutation::Ban { player, .. }
            | Mutation::Pardon { player } => player,
            Mutation::BanIp { ip, .. } | Mutation::PardonIp { ip } => ip,
        }
    }
}

/// How a mutation was carried out, which the UI reports back to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub enum MutationRoute {
    /// Sent to the running server on stdin; the file was then re-read.
    Command,
    /// Written straight to the JSON file, because the server is stopped.
    File,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct MutationReport {
    pub route: MutationRoute,
    pub command: Option<String>,
    pub lists: PlayerLists,
}

/// How long to wait for a running server to write the file back after a command.
const COMMAND_SETTLE: std::time::Duration = std::time::Duration::from_millis(600);

/// The single gate. Every op/whitelist/ban change in this app goes through here.
pub async fn mutate(
    state: &AppState,
    id: i64,
    mutation: Mutation,
) -> AppResult<MutationReport> {
    let row = instance::get(&state.db, id).await?;
    let dir = row.path_buf();
    if !dir.is_dir() {
        return Err(AppError::FolderMissing {
            name: row.name,
            path: dir,
        });
    }

    let subject = mutation.subject().trim().to_string();
    if subject.is_empty() {
        return Err(AppError::Other("no player or address was given".into()));
    }

    let running = state.supervisor.is_running(&row.uuid);
    let route = if running {
        // The server owns the lists while it runs: ask it, do not fight it.
        let command = mutation.console_command();
        supervisor::send_command(state, id, &command).await?;
        // Give the server a moment to write the file back before re-reading.
        tokio::time::sleep(COMMAND_SETTLE).await;
        MutationRoute::Command
    } else {
        files::apply_offline(&dir, &mutation).await?;
        MutationRoute::File
    };

    record_event(
        &state.db,
        id,
        "players",
        Some(&format!(
            "{} ({})",
            mutation.console_command(),
            match route {
                MutationRoute::Command => "via console",
                MutationRoute::File => "written to file",
            }
        )),
    )
    .await?;

    Ok(MutationReport {
        route,
        command: matches!(route, MutationRoute::Command).then(|| mutation.console_command()),
        lists: lists(state, id).await?,
    })
}

/// Reads all four lists plus the seen-player history.
pub async fn lists(state: &AppState, id: i64) -> AppResult<PlayerLists> {
    let row = instance::get(&state.db, id).await?;
    let dir = row.path_buf();
    if !dir.is_dir() {
        return Err(AppError::FolderMissing {
            name: row.name,
            path: dir,
        });
    }

    let seen = sqlx::query_as::<_, SeenPlayer>(
        "SELECT uuid, name, first_seen, last_seen FROM players_seen
         WHERE instance_id = ? ORDER BY last_seen DESC",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;

    let whitelist_enabled = crate::config::read(&dir)
        .await?
        .get("white-list")
        .map(|value| value == "true")
        .unwrap_or(false);

    Ok(PlayerLists {
        ops: files::read_list(&dir, ListKind::Ops).await?,
        whitelist: files::read_list(&dir, ListKind::Whitelist).await?,
        banned_players: files::read_list(&dir, ListKind::BannedPlayers).await?,
        banned_ips: files::read_list(&dir, ListKind::BannedIps).await?,
        seen,
        whitelist_enabled,
        running: state.supervisor.is_running(&row.uuid),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_match_what_a_server_accepts() {
        assert_eq!(
            Mutation::Op {
                player: "Notch".into(),
                level: Some(4)
            }
            .console_command(),
            "op Notch"
        );
        assert_eq!(
            Mutation::Deop {
                player: "Notch".into()
            }
            .console_command(),
            "deop Notch"
        );
        assert_eq!(
            Mutation::WhitelistAdd {
                player: "Notch".into()
            }
            .console_command(),
            "whitelist add Notch"
        );
        assert_eq!(
            Mutation::WhitelistRemove {
                player: "Notch".into()
            }
            .console_command(),
            "whitelist remove Notch"
        );
        assert_eq!(
            Mutation::Pardon {
                player: "Notch".into()
            }
            .console_command(),
            "pardon Notch"
        );
        assert_eq!(
            Mutation::PardonIp {
                ip: "1.2.3.4".into()
            }
            .console_command(),
            "pardon-ip 1.2.3.4"
        );
    }

    #[test]
    fn a_ban_reason_is_appended_only_when_there_is_one() {
        assert_eq!(
            Mutation::Ban {
                player: "Griefer".into(),
                reason: Some("blew up spawn".into())
            }
            .console_command(),
            "ban Griefer blew up spawn"
        );
        assert_eq!(
            Mutation::Ban {
                player: "Griefer".into(),
                reason: None
            }
            .console_command(),
            "ban Griefer"
        );
        assert_eq!(
            Mutation::Ban {
                player: "Griefer".into(),
                reason: Some("   ".into())
            }
            .console_command(),
            "ban Griefer",
            "a blank reason is not sent as an argument"
        );
        assert_eq!(
            Mutation::BanIp {
                ip: "1.2.3.4".into(),
                reason: Some("spam".into())
            }
            .console_command(),
            "ban-ip 1.2.3.4 spam"
        );
    }

    #[test]
    fn every_mutation_names_its_list_and_subject() {
        assert_eq!(
            Mutation::Op {
                player: "A".into(),
                level: None
            }
            .list(),
            ListKind::Ops
        );
        assert_eq!(
            Mutation::WhitelistAdd { player: "A".into() }.list(),
            ListKind::Whitelist
        );
        assert_eq!(
            Mutation::Ban {
                player: "A".into(),
                reason: None
            }
            .list(),
            ListKind::BannedPlayers
        );
        assert_eq!(
            Mutation::BanIp {
                ip: "1.2.3.4".into(),
                reason: None
            }
            .list(),
            ListKind::BannedIps
        );
        assert_eq!(
            Mutation::BanIp {
                ip: "1.2.3.4".into(),
                reason: None
            }
            .subject(),
            "1.2.3.4"
        );
    }
}
