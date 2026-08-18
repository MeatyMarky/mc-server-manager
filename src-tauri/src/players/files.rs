//! The four JSON files a server keeps its player lists in.
//!
//! These are only written while the server is stopped — [`super::mutate`] is the
//! only caller, and it routes live instances through the console instead.
//!
//! A player added while the server is stopped needs a UUID, and the server will
//! not fix a wrong one. Mojang's API is asked first; failing that (offline, or
//! an offline-mode server) the offline UUID is derived the same way the server
//! derives it, so the entry still matches the player who joins.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{AppError, AppResult, IoContext};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    Ops,
    Whitelist,
    BannedPlayers,
    BannedIps,
}

impl ListKind {
    pub fn file_name(self) -> &'static str {
        match self {
            ListKind::Ops => "ops.json",
            ListKind::Whitelist => "whitelist.json",
            ListKind::BannedPlayers => "banned-players.json",
            ListKind::BannedIps => "banned-ips.json",
        }
    }

    pub fn path(self, instance_path: &Path) -> PathBuf {
        instance_path.join(self.file_name())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct OpEntry {
    pub uuid: String,
    pub name: String,
    #[serde(default = "default_level")]
    #[ts(type = "number")]
    pub level: i64,
    #[serde(rename = "bypassesPlayerLimit", default)]
    pub bypasses_player_limit: bool,
}

fn default_level() -> i64 {
    4
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct WhitelistEntry {
    pub uuid: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct BannedPlayer {
    pub uuid: String,
    pub name: String,
    #[serde(default)]
    pub created: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub expires: String,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct BannedIp {
    pub ip: String,
    #[serde(default)]
    pub created: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub expires: String,
    #[serde(default)]
    pub reason: String,
}

/// Reads one list. A missing or unreadable file is an empty list, never an error:
/// a server that has never run has none of these files.
pub async fn read_list<T: serde::de::DeserializeOwned>(
    instance_path: &Path,
    kind: ListKind,
) -> AppResult<Vec<T>> {
    let path = kind.path(instance_path);
    let text = match tokio::fs::read_to_string(&path).await {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(AppError::io("read player list", &path, err)),
    };
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&text).map_err(|e| {
        AppError::Other(format!("{} is not valid JSON: {e}", path.display()))
    })
}

/// Pretty-printed like the server writes them, atomically.
pub async fn write_list<T: Serialize>(
    instance_path: &Path,
    kind: ListKind,
    entries: &[T],
) -> AppResult<()> {
    let path = kind.path(instance_path);
    let json = serde_json::to_string_pretty(entries)?;
    let temp = path.with_extension("json.tmp");
    tokio::fs::write(&temp, json.as_bytes())
        .await
        .ctx("write player list", &temp)?;
    tokio::fs::rename(&temp, &path)
        .await
        .ctx("replace player list", &path)?;
    Ok(())
}

/// Minecraft's offline UUID: version 3 over `OfflinePlayer:<name>`.
///
/// This is what an offline-mode server assigns, so an entry written with it
/// matches the player who later joins.
pub fn offline_uuid(name: &str) -> String {
    let digest = md5::Md5::digest(format!("OfflinePlayer:{name}").as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest);
    // Set the version (3) and variant (RFC 4122) bits.
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

use md5::Digest as _;

/// Looks a name up with Mojang, falling back to the offline UUID.
pub async fn resolve_uuid(http: &crate::http::Http, name: &str) -> (String, bool) {
    use crate::http::Fetch;

    let url = format!("https://api.mojang.com/users/profiles/minecraft/{name}");
    match http.get_text(&url).await {
        Ok(body) => match serde_json::from_str::<serde_json::Value>(&body) {
            Ok(value) => match value.get("id").and_then(|id| id.as_str()) {
                Some(compact) if compact.len() == 32 => (dashed(compact), true),
                _ => (offline_uuid(name), false),
            },
            Err(_) => (offline_uuid(name), false),
        },
        Err(_) => (offline_uuid(name), false),
    }
}

/// Mojang returns UUIDs without dashes.
pub fn dashed(compact: &str) -> String {
    if compact.len() != 32 {
        return compact.to_string();
    }
    format!(
        "{}-{}-{}-{}-{}",
        &compact[0..8],
        &compact[8..12],
        &compact[12..16],
        &compact[16..20],
        &compact[20..32]
    )
}

fn now() -> String {
    // The format the server itself writes into these files.
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S %z").to_string()
}

/// Applies a mutation to the files directly. Only valid while the server is
/// stopped; [`super::mutate`] enforces that.
pub async fn apply_offline(instance_path: &Path, mutation: &super::Mutation) -> AppResult<()> {
    use super::Mutation;

    match mutation {
        Mutation::Op { player, level } => {
            let mut ops: Vec<OpEntry> = read_list(instance_path, ListKind::Ops).await?;
            let uuid = uuid_for(instance_path, player).await;
            if let Some(existing) = ops.iter_mut().find(|entry| same_player(&entry.name, player)) {
                existing.level = level.unwrap_or(existing.level);
            } else {
                ops.push(OpEntry {
                    uuid,
                    name: player.clone(),
                    level: level.unwrap_or(4),
                    bypasses_player_limit: false,
                });
            }
            write_list(instance_path, ListKind::Ops, &ops).await
        }
        Mutation::Deop { player } => {
            let mut ops: Vec<OpEntry> = read_list(instance_path, ListKind::Ops).await?;
            ops.retain(|entry| !same_player(&entry.name, player));
            write_list(instance_path, ListKind::Ops, &ops).await
        }
        Mutation::WhitelistAdd { player } => {
            let mut list: Vec<WhitelistEntry> =
                read_list(instance_path, ListKind::Whitelist).await?;
            if !list.iter().any(|entry| same_player(&entry.name, player)) {
                list.push(WhitelistEntry {
                    uuid: uuid_for(instance_path, player).await,
                    name: player.clone(),
                });
            }
            write_list(instance_path, ListKind::Whitelist, &list).await
        }
        Mutation::WhitelistRemove { player } => {
            let mut list: Vec<WhitelistEntry> =
                read_list(instance_path, ListKind::Whitelist).await?;
            list.retain(|entry| !same_player(&entry.name, player));
            write_list(instance_path, ListKind::Whitelist, &list).await
        }
        Mutation::Ban { player, reason } => {
            let mut list: Vec<BannedPlayer> =
                read_list(instance_path, ListKind::BannedPlayers).await?;
            if !list.iter().any(|entry| same_player(&entry.name, player)) {
                list.push(BannedPlayer {
                    uuid: uuid_for(instance_path, player).await,
                    name: player.clone(),
                    created: now(),
                    source: "Server Manager".to_string(),
                    expires: "forever".to_string(),
                    reason: reason
                        .clone()
                        .filter(|text| !text.trim().is_empty())
                        .unwrap_or_else(|| "Banned by an operator.".to_string()),
                });
            }
            write_list(instance_path, ListKind::BannedPlayers, &list).await
        }
        Mutation::Pardon { player } => {
            let mut list: Vec<BannedPlayer> =
                read_list(instance_path, ListKind::BannedPlayers).await?;
            list.retain(|entry| !same_player(&entry.name, player));
            write_list(instance_path, ListKind::BannedPlayers, &list).await
        }
        Mutation::BanIp { ip, reason } => {
            let mut list: Vec<BannedIp> = read_list(instance_path, ListKind::BannedIps).await?;
            if !list.iter().any(|entry| entry.ip == *ip) {
                list.push(BannedIp {
                    ip: ip.clone(),
                    created: now(),
                    source: "Server Manager".to_string(),
                    expires: "forever".to_string(),
                    reason: reason
                        .clone()
                        .filter(|text| !text.trim().is_empty())
                        .unwrap_or_else(|| "Banned by an operator.".to_string()),
                });
            }
            write_list(instance_path, ListKind::BannedIps, &list).await
        }
        Mutation::PardonIp { ip } => {
            let mut list: Vec<BannedIp> = read_list(instance_path, ListKind::BannedIps).await?;
            list.retain(|entry| entry.ip != *ip);
            write_list(instance_path, ListKind::BannedIps, &list).await
        }
    }
}

/// Player names are case-insensitive to Minecraft.
fn same_player(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b.trim())
}

/// Reuses a UUID already known for this player (from another list or from the
/// console history file) before falling back to the offline one. Mojang lookups
/// happen in the command layer, which has the HTTP client.
async fn uuid_for(instance_path: &Path, player: &str) -> String {
    for kind in [ListKind::Ops, ListKind::Whitelist] {
        if let Ok(entries) = read_list::<serde_json::Value>(instance_path, kind).await {
            for entry in entries {
                let matches = entry
                    .get("name")
                    .and_then(|name| name.as_str())
                    .map(|name| same_player(name, player))
                    .unwrap_or(false);
                if matches {
                    if let Some(uuid) = entry.get("uuid").and_then(|uuid| uuid.as_str()) {
                        return uuid.to_string();
                    }
                }
            }
        }
    }
    offline_uuid(player)
}

#[cfg(test)]
mod tests {
    use super::super::Mutation;
    use super::*;

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn file_names_match_what_the_server_writes() {
        assert_eq!(ListKind::Ops.file_name(), "ops.json");
        assert_eq!(ListKind::Whitelist.file_name(), "whitelist.json");
        assert_eq!(ListKind::BannedPlayers.file_name(), "banned-players.json");
        assert_eq!(ListKind::BannedIps.file_name(), "banned-ips.json");
    }

    #[test]
    fn offline_uuids_match_the_servers_own_derivation() {
        // The known value an offline-mode server assigns to "Notch".
        assert_eq!(offline_uuid("Notch"), "b50ad385-829d-3141-a216-7e7d7539ba7f");

        // Shape: version 3 (the 13th hex digit) and the RFC 4122 variant.
        let generated = offline_uuid("SomeoneElse");
        assert_eq!(generated.len(), 36);
        assert_eq!(generated.chars().nth(14), Some('3'), "UUID version 3");
        assert!(matches!(
            generated.chars().nth(19),
            Some('8') | Some('9') | Some('a') | Some('b')
        ));

        // Stable and case-sensitive, like Minecraft's own hashing.
        assert_eq!(offline_uuid("Notch"), offline_uuid("Notch"));
        assert_ne!(offline_uuid("Notch"), offline_uuid("notch"));
    }

    #[test]
    fn mojang_uuids_gain_their_dashes() {
        assert_eq!(
            dashed("069a79f444e94726a5befca90e38aaf5"),
            "069a79f4-44e9-4726-a5be-fca90e38aaf5"
        );
        assert_eq!(dashed("short"), "short");
    }

    #[tokio::test]
    async fn missing_files_read_as_empty_lists() {
        let dir = dir();
        let ops: Vec<OpEntry> = read_list(dir.path(), ListKind::Ops).await.unwrap();
        assert!(ops.is_empty());
    }

    #[tokio::test]
    async fn a_real_ops_file_parses() {
        let dir = dir();
        std::fs::write(
            ListKind::Ops.path(dir.path()),
            r#"[{"uuid":"069a79f4-44e9-4726-a5be-fca90e38aaf5","name":"Notch","level":4,"bypassesPlayerLimit":false}]"#,
        )
        .unwrap();

        let ops: Vec<OpEntry> = read_list(dir.path(), ListKind::Ops).await.unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].name, "Notch");
        assert_eq!(ops[0].level, 4);
        assert!(!ops[0].bypasses_player_limit);
    }

    #[tokio::test]
    async fn adding_and_removing_an_op_round_trips() {
        let dir = dir();
        apply_offline(
            dir.path(),
            &Mutation::Op {
                player: "Notch".into(),
                level: Some(3),
            },
        )
        .await
        .unwrap();

        let ops: Vec<OpEntry> = read_list(dir.path(), ListKind::Ops).await.unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].level, 3);
        assert_eq!(ops[0].uuid, offline_uuid("Notch"));

        // Adding again updates the level rather than duplicating the entry.
        apply_offline(
            dir.path(),
            &Mutation::Op {
                player: "notch".into(),
                level: Some(2),
            },
        )
        .await
        .unwrap();
        let ops: Vec<OpEntry> = read_list(dir.path(), ListKind::Ops).await.unwrap();
        assert_eq!(ops.len(), 1, "names are case-insensitive");
        assert_eq!(ops[0].level, 2);

        apply_offline(
            dir.path(),
            &Mutation::Deop {
                player: "NOTCH".into(),
            },
        )
        .await
        .unwrap();
        let ops: Vec<OpEntry> = read_list(dir.path(), ListKind::Ops).await.unwrap();
        assert!(ops.is_empty());
    }

    #[tokio::test]
    async fn whitelisting_reuses_a_uuid_already_known_for_that_player() {
        let dir = dir();
        std::fs::write(
            ListKind::Ops.path(dir.path()),
            r#"[{"uuid":"069a79f4-44e9-4726-a5be-fca90e38aaf5","name":"Notch","level":4,"bypassesPlayerLimit":false}]"#,
        )
        .unwrap();

        apply_offline(dir.path(), &Mutation::WhitelistAdd { player: "Notch".into() })
            .await
            .unwrap();

        let list: Vec<WhitelistEntry> = read_list(dir.path(), ListKind::Whitelist).await.unwrap();
        assert_eq!(list[0].uuid, "069a79f4-44e9-4726-a5be-fca90e38aaf5");
    }

    #[tokio::test]
    async fn bans_record_a_reason_and_pardons_remove_them() {
        let dir = dir();
        apply_offline(
            dir.path(),
            &Mutation::Ban {
                player: "Griefer".into(),
                reason: Some("blew up spawn".into()),
            },
        )
        .await
        .unwrap();

        let list: Vec<BannedPlayer> = read_list(dir.path(), ListKind::BannedPlayers).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].reason, "blew up spawn");
        assert_eq!(list[0].expires, "forever");
        assert!(!list[0].created.is_empty());

        apply_offline(dir.path(), &Mutation::Pardon { player: "griefer".into() })
            .await
            .unwrap();
        let list: Vec<BannedPlayer> = read_list(dir.path(), ListKind::BannedPlayers).await.unwrap();
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn a_ban_without_a_reason_gets_the_servers_default_wording() {
        let dir = dir();
        apply_offline(
            dir.path(),
            &Mutation::Ban {
                player: "Someone".into(),
                reason: None,
            },
        )
        .await
        .unwrap();
        let list: Vec<BannedPlayer> = read_list(dir.path(), ListKind::BannedPlayers).await.unwrap();
        assert_eq!(list[0].reason, "Banned by an operator.");
    }

    #[tokio::test]
    async fn ip_bans_are_kept_separately_and_are_exact() {
        let dir = dir();
        apply_offline(
            dir.path(),
            &Mutation::BanIp {
                ip: "203.0.113.7".into(),
                reason: None,
            },
        )
        .await
        .unwrap();
        apply_offline(
            dir.path(),
            &Mutation::BanIp {
                ip: "203.0.113.7".into(),
                reason: None,
            },
        )
        .await
        .unwrap();

        let list: Vec<BannedIp> = read_list(dir.path(), ListKind::BannedIps).await.unwrap();
        assert_eq!(list.len(), 1, "banning twice does not duplicate");

        apply_offline(dir.path(), &Mutation::PardonIp { ip: "203.0.113.7".into() })
            .await
            .unwrap();
        let list: Vec<BannedIp> = read_list(dir.path(), ListKind::BannedIps).await.unwrap();
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn writes_are_atomic_and_leave_no_temp_file() {
        let dir = dir();
        apply_offline(dir.path(), &Mutation::WhitelistAdd { player: "A".into() })
            .await
            .unwrap();

        let leftovers: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.contains("tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp files: {leftovers:?}");
    }

    #[tokio::test]
    async fn a_corrupt_file_is_reported_rather_than_silently_replaced() {
        let dir = dir();
        std::fs::write(ListKind::Ops.path(dir.path()), b"{not json").unwrap();
        let result: AppResult<Vec<OpEntry>> = read_list(dir.path(), ListKind::Ops).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not valid JSON"));
    }
}
