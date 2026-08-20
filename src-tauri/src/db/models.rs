//! Row structs and the DTOs handed to the UI.
//!
//! `Instance` mirrors the `instances` table one-to-one. `InstanceView` is what
//! the frontend sees: JSON-typed columns decoded, plus the runtime status that
//! is never persisted.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type, TS)]
#[sqlx(rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub enum ServerType {
    Vanilla,
    Paper,
    Purpur,
    Fabric,
    Forge,
    NeoForge,
}

impl ServerType {
    /// Bukkit-family servers load `plugins/`, everything else loads `mods/`.
    pub fn content_dir_name(self) -> &'static str {
        match self {
            ServerType::Paper | ServerType::Purpur => "plugins",
            _ => "mods",
        }
    }

    pub fn loads_mods(self) -> bool {
        !matches!(self, ServerType::Vanilla)
    }

    /// How this type is written in a sentence: "1.16.5 Forge", "1.21 Paper".
    pub fn label(self) -> &'static str {
        match self {
            ServerType::Vanilla => "Vanilla",
            ServerType::Paper => "Paper",
            ServerType::Purpur => "Purpur",
            ServerType::Fabric => "Fabric",
            ServerType::Forge => "Forge",
            ServerType::NeoForge => "NeoForge",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ServerType::Vanilla => "vanilla",
            ServerType::Paper => "paper",
            ServerType::Purpur => "purpur",
            ServerType::Fabric => "fabric",
            ServerType::Forge => "forge",
            ServerType::NeoForge => "neo_forge",
        }
    }
}

/// How the server is launched. Forge/NeoForge >= 1.17 have no runnable single
/// jar; they ship `libraries/` plus an `@…_args.txt` argument file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type, TS)]
#[sqlx(rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub enum LaunchKind {
    /// `java -jar <launch_target>`
    Jar,
    /// `java @<launch_target>` (Forge/NeoForge args file)
    ArgsFile,
    /// `run.sh` / `run.bat` produced by an installer
    Script,
}

/// Runtime status. Only ever derived — never read back from the database as truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub enum InstanceStatus {
    Stopped,
    Starting,
    Running,
    Stopping,
    Crashed,
    /// Alive, but this app does not own its stdio: "running, console unavailable".
    Unmanaged,
    /// The instance folder is gone or was moved. Recoverable, never an error.
    Missing,
}

impl InstanceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            InstanceStatus::Stopped => "stopped",
            InstanceStatus::Starting => "starting",
            InstanceStatus::Running => "running",
            InstanceStatus::Stopping => "stopping",
            InstanceStatus::Crashed => "crashed",
            InstanceStatus::Unmanaged => "unmanaged",
            InstanceStatus::Missing => "missing",
        }
    }

    pub fn is_live(self) -> bool {
        matches!(
            self,
            InstanceStatus::Starting
                | InstanceStatus::Running
                | InstanceStatus::Stopping
                | InstanceStatus::Unmanaged
        )
    }
}

/// One row of `instances`.
#[derive(Debug, Clone, FromRow)]
pub struct Instance {
    pub id: i64,
    pub uuid: String,
    pub name: String,
    pub path: String,
    pub server_type: ServerType,
    pub mc_version: String,
    pub loader_version: Option<String>,
    pub launch_kind: LaunchKind,
    pub launch_target: Option<String>,
    pub java_path: Option<String>,
    pub java_major: Option<i64>,
    pub jvm_args: String,
    pub server_args: String,
    pub min_ram_mb: i64,
    pub max_ram_mb: i64,
    pub eula_accepted: bool,
    pub eula_accepted_at: Option<String>,
    pub auto_start: bool,
    pub auto_restart: bool,
    pub restart_max: i64,
    pub restart_window_s: i64,
    pub stop_timeout_s: i64,
    pub rcon_enabled: bool,
    pub rcon_port: Option<i64>,
    pub rcon_password: Option<String>,
    pub color: Option<String>,
    pub notes: Option<String>,
    pub last_status: Option<String>,
    pub last_exit_code: Option<i64>,
    pub last_started_at: Option<String>,
    pub last_stopped_at: Option<String>,
    pub pid: Option<i64>,
    pub process_start_time: Option<i64>,
    pub installed_artifact_url: Option<String>,
    pub installed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl Instance {
    pub fn path_buf(&self) -> std::path::PathBuf {
        std::path::PathBuf::from(&self.path)
    }

    pub fn folder_exists(&self) -> bool {
        self.path_buf().is_dir()
    }

    fn string_list(raw: &str) -> Vec<String> {
        serde_json::from_str(raw).unwrap_or_default()
    }

    /// `status` comes from the supervisor; a missing folder always wins over it,
    /// because nothing else can be done with the instance until it is located.
    pub fn to_view(&self, status: InstanceStatus) -> InstanceView {
        let folder_exists = self.folder_exists();
        let status = if folder_exists {
            status
        } else {
            InstanceStatus::Missing
        };
        InstanceView {
            id: self.id,
            uuid: self.uuid.clone(),
            name: self.name.clone(),
            path: self.path.clone(),
            folder_exists,
            status,
            server_type: self.server_type,
            mc_version: self.mc_version.clone(),
            loader_version: self.loader_version.clone(),
            launch_kind: self.launch_kind,
            launch_target: self.launch_target.clone(),
            java_path: self.java_path.clone(),
            java_major: self.java_major,
            jvm_args: Self::string_list(&self.jvm_args),
            server_args: Self::string_list(&self.server_args),
            min_ram_mb: self.min_ram_mb,
            max_ram_mb: self.max_ram_mb,
            eula_accepted: self.eula_accepted,
            eula_accepted_at: self.eula_accepted_at.clone(),
            auto_start: self.auto_start,
            auto_restart: self.auto_restart,
            restart_max: self.restart_max,
            restart_window_s: self.restart_window_s,
            stop_timeout_s: self.stop_timeout_s,
            content_dir: self.server_type.content_dir_name().to_string(),
            color: self.color.clone(),
            notes: self.notes.clone(),
            last_exit_code: self.last_exit_code,
            last_started_at: self.last_started_at.clone(),
            last_stopped_at: self.last_stopped_at.clone(),
            pid: self.pid,
            installed_at: self.installed_at.clone(),
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
        }
    }

    /// The recovery mirror written to `.msm/instance.json`. Read only on import
    /// or when a folder has no matching database row.
    pub fn to_manifest(&self) -> InstanceManifest {
        InstanceManifest {
            schema: 1,
            uuid: self.uuid.clone(),
            name: self.name.clone(),
            server_type: self.server_type,
            mc_version: self.mc_version.clone(),
            loader_version: self.loader_version.clone(),
            launch_kind: self.launch_kind,
            launch_target: self.launch_target.clone(),
            jvm_args: Self::string_list(&self.jvm_args),
            server_args: Self::string_list(&self.server_args),
            min_ram_mb: self.min_ram_mb,
            max_ram_mb: self.max_ram_mb,
            java_path: self.java_path.clone(),
            eula_accepted: self.eula_accepted,
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct InstanceView {
    #[ts(type = "number")]
    pub id: i64,
    pub uuid: String,
    pub name: String,
    pub path: String,
    pub folder_exists: bool,
    pub status: InstanceStatus,
    pub server_type: ServerType,
    pub mc_version: String,
    pub loader_version: Option<String>,
    pub launch_kind: LaunchKind,
    pub launch_target: Option<String>,
    pub java_path: Option<String>,
    #[ts(type = "number | null")]
    pub java_major: Option<i64>,
    pub jvm_args: Vec<String>,
    pub server_args: Vec<String>,
    #[ts(type = "number")]
    pub min_ram_mb: i64,
    #[ts(type = "number")]
    pub max_ram_mb: i64,
    pub eula_accepted: bool,
    pub eula_accepted_at: Option<String>,
    pub auto_start: bool,
    pub auto_restart: bool,
    #[ts(type = "number")]
    pub restart_max: i64,
    #[ts(type = "number")]
    pub restart_window_s: i64,
    #[ts(type = "number")]
    pub stop_timeout_s: i64,
    /// `mods` or `plugins`, derived from the server type.
    pub content_dir: String,
    pub color: Option<String>,
    pub notes: Option<String>,
    #[ts(type = "number | null")]
    pub last_exit_code: Option<i64>,
    pub last_started_at: Option<String>,
    pub last_stopped_at: Option<String>,
    #[ts(type = "number | null")]
    pub pid: Option<i64>,
    /// When a server was last installed into this folder; null means the
    /// instance has no server files yet.
    pub installed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// `.msm/instance.json`. The database row is authoritative during normal
/// operation; this file exists so a lost database or a copied folder is
/// recoverable, and it is only read on import.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct InstanceManifest {
    pub schema: u32,
    pub uuid: String,
    pub name: String,
    pub server_type: ServerType,
    pub mc_version: String,
    pub loader_version: Option<String>,
    pub launch_kind: LaunchKind,
    pub launch_target: Option<String>,
    pub jvm_args: Vec<String>,
    pub server_args: Vec<String>,
    #[ts(type = "number")]
    pub min_ram_mb: i64,
    #[ts(type = "number")]
    pub max_ram_mb: i64,
    pub java_path: Option<String>,
    pub eula_accepted: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Default JVM arguments. Aikar's flags land in Phase 2 together with the heap
/// sizing UI; these are the safe minimum that works on every supported Java.
pub fn default_jvm_args() -> Vec<String> {
    vec![
        "-XX:+UseG1GC".to_string(),
        "-XX:+ParallelRefProcEnabled".to_string(),
        "-XX:+UnlockExperimentalVMOptions".to_string(),
        "-XX:+DisableExplicitGC".to_string(),
    ]
}

pub fn default_server_args() -> Vec<String> {
    vec!["--nogui".to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_dir_follows_server_family() {
        assert_eq!(ServerType::Paper.content_dir_name(), "plugins");
        assert_eq!(ServerType::Purpur.content_dir_name(), "plugins");
        assert_eq!(ServerType::Fabric.content_dir_name(), "mods");
        assert_eq!(ServerType::Forge.content_dir_name(), "mods");
        assert_eq!(ServerType::NeoForge.content_dir_name(), "mods");
        assert_eq!(ServerType::Vanilla.content_dir_name(), "mods");
        assert!(!ServerType::Vanilla.loads_mods());
    }

    #[test]
    fn missing_folder_overrides_reported_status() {
        let inst = sample_instance("Z:/definitely/not/here");
        assert_eq!(
            inst.to_view(InstanceStatus::Running).status,
            InstanceStatus::Missing
        );
    }

    #[test]
    fn view_decodes_json_argument_lists() {
        let mut inst = sample_instance("Z:/nope");
        inst.jvm_args = r#"["-Xmx4G","-XX:+UseG1GC"]"#.into();
        inst.server_args = r#"["--nogui"]"#.into();
        let view = inst.to_view(InstanceStatus::Stopped);
        assert_eq!(view.jvm_args, vec!["-Xmx4G", "-XX:+UseG1GC"]);
        assert_eq!(view.server_args, vec!["--nogui"]);
    }

    #[test]
    fn malformed_argument_json_degrades_to_empty_not_panic() {
        let mut inst = sample_instance("Z:/nope");
        inst.jvm_args = "not json".into();
        assert!(inst.to_view(InstanceStatus::Stopped).jvm_args.is_empty());
    }

    fn sample_instance(path: &str) -> Instance {
        Instance {
            id: 1,
            uuid: "u".into(),
            name: "Test".into(),
            path: path.into(),
            server_type: ServerType::Paper,
            mc_version: "1.21.4".into(),
            loader_version: None,
            launch_kind: LaunchKind::Jar,
            launch_target: Some("server.jar".into()),
            java_path: None,
            java_major: Some(21),
            jvm_args: "[]".into(),
            server_args: "[]".into(),
            min_ram_mb: 1024,
            max_ram_mb: 4096,
            eula_accepted: false,
            eula_accepted_at: None,
            auto_start: false,
            auto_restart: false,
            restart_max: 3,
            restart_window_s: 600,
            stop_timeout_s: 60,
            rcon_enabled: false,
            rcon_port: None,
            rcon_password: None,
            color: None,
            notes: None,
            last_status: None,
            last_exit_code: None,
            last_started_at: None,
            last_stopped_at: None,
            pid: None,
            process_start_time: None,
            installed_artifact_url: None,
            installed_at: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        }
    }
}
