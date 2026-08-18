//! The mod source abstraction.
//!
//! Everything above this boundary — resolution, install, the UI — speaks only
//! these types. Modrinth is the one implementation today; CurseForge is meant to
//! arrive as another `impl ModSource`, not as a refactor, so nothing here may
//! mention a provider's own field names or id formats.

use std::future::Future;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::db::models::ServerType;
use crate::error::AppResult;

/// Which source a project came from. Stored in `mods.source`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type, TS)]
#[sqlx(rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub enum SourceId {
    Modrinth,
    /// Reserved: a second implementation, not a second code path.
    CurseForge,
    /// A jar the user supplied.
    Local,
}

impl SourceId {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceId::Modrinth => "modrinth",
            SourceId::CurseForge => "curse_forge",
            SourceId::Local => "local",
        }
    }
}

/// The loader a server runs, as far as mod compatibility is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub enum Loader {
    Fabric,
    Forge,
    NeoForge,
    /// Bukkit-family plugins: Paper and Purpur load the same jars.
    Paper,
}

impl Loader {
    /// The loader an instance uses, or `None` for vanilla, which loads nothing.
    pub fn for_server_type(server_type: ServerType) -> Option<Self> {
        match server_type {
            ServerType::Fabric => Some(Loader::Fabric),
            ServerType::Forge => Some(Loader::Forge),
            ServerType::NeoForge => Some(Loader::NeoForge),
            ServerType::Paper | ServerType::Purpur => Some(Loader::Paper),
            ServerType::Vanilla => None,
        }
    }

    /// The identifier the source's API uses for this loader.
    pub fn as_str(self) -> &'static str {
        match self {
            Loader::Fabric => "fabric",
            Loader::Forge => "forge",
            Loader::NeoForge => "neoforge",
            Loader::Paper => "paper",
        }
    }

    /// Loaders whose jars this instance can also load.
    ///
    /// Paper accepts plugins published for Bukkit, Spigot and Paper; NeoForge
    /// still loads a good deal of Forge-tagged content, but the reverse is not
    /// true, so the widening is one-directional.
    pub fn accepted(self) -> Vec<&'static str> {
        match self {
            Loader::Fabric => vec!["fabric"],
            Loader::Forge => vec!["forge"],
            Loader::NeoForge => vec!["neoforge", "forge"],
            Loader::Paper => vec!["paper", "spigot", "bukkit", "folia"],
        }
    }

    /// Where jars for this loader are installed.
    pub fn content_dir(self) -> &'static str {
        match self {
            Loader::Paper => "plugins",
            _ => "mods",
        }
    }
}

/// A project as a search result or a detail view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct Project {
    pub source: SourceId,
    /// The source's own id, opaque to everything above this boundary.
    pub id: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub author: Option<String>,
    #[ts(type = "number | null")]
    pub downloads: Option<i64>,
    pub icon_url: Option<String>,
    pub page_url: Option<String>,
    pub categories: Vec<String>,
    /// Loader identifiers the project publishes for.
    pub loaders: Vec<String>,
}

/// One downloadable file of a version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct SourceFile {
    pub url: String,
    pub file_name: String,
    pub sha1: Option<String>,
    pub sha512: Option<String>,
    #[ts(type = "number | null")]
    pub size: Option<u64>,
    /// The file to install when a version ships several.
    pub primary: bool,
}

/// How a dependency relates to the version that declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub enum DependencyKind {
    Required,
    Optional,
    Incompatible,
    /// Bundled inside the jar; nothing to install.
    Embedded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct Dependency {
    pub kind: DependencyKind,
    /// One of these is always set.
    pub project_id: Option<String>,
    pub version_id: Option<String>,
}

/// One release of a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct SourceVersion {
    pub source: SourceId,
    pub id: String,
    pub project_id: String,
    pub name: String,
    /// The version string the author chose, e.g. "0.5.11+mc1.21.4".
    pub version_number: String,
    /// release | beta | alpha
    pub channel: String,
    pub published: Option<String>,
    pub game_versions: Vec<String>,
    pub loaders: Vec<String>,
    pub files: Vec<SourceFile>,
    pub dependencies: Vec<Dependency>,
}

impl SourceVersion {
    /// The file to install: the one marked primary, else the first.
    pub fn primary_file(&self) -> Option<&SourceFile> {
        self.files
            .iter()
            .find(|file| file.primary)
            .or_else(|| self.files.first())
    }

    pub fn is_stable(&self) -> bool {
        self.channel.eq_ignore_ascii_case("release")
    }
}

/// What to search for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct SearchQuery {
    pub text: String,
    /// Loader identifiers to accept; empty means any.
    pub loaders: Vec<String>,
    /// Game versions to accept; empty means any.
    pub game_versions: Vec<String>,
    #[ts(type = "number | null")]
    pub limit: Option<u32>,
    #[ts(type = "number | null")]
    pub offset: Option<u32>,
}

/// Which versions of a project are wanted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VersionFilter {
    pub loaders: Vec<String>,
    pub game_versions: Vec<String>,
}

/// A source of mods and plugins.
///
/// Implementations own their API's shapes entirely; callers see only the types
/// above. Adding CurseForge means adding an implementation and a `SourceId`
/// variant, and nothing else.
pub trait ModSource: Send + Sync {
    fn id(&self) -> SourceId;

    fn search(&self, query: &SearchQuery)
        -> impl Future<Output = AppResult<Vec<Project>>> + Send;

    fn project(&self, project_id: &str) -> impl Future<Output = AppResult<Project>> + Send;

    /// Versions of a project, newest release first.
    fn versions(
        &self,
        project_id: &str,
        filter: &VersionFilter,
    ) -> impl Future<Output = AppResult<Vec<SourceVersion>>> + Send;

    fn version(&self, version_id: &str) -> impl Future<Output = AppResult<SourceVersion>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loaders_follow_the_server_type_not_the_file() {
        assert_eq!(Loader::for_server_type(ServerType::Paper), Some(Loader::Paper));
        assert_eq!(Loader::for_server_type(ServerType::Purpur), Some(Loader::Paper));
        assert_eq!(Loader::for_server_type(ServerType::Fabric), Some(Loader::Fabric));
        assert_eq!(Loader::for_server_type(ServerType::Forge), Some(Loader::Forge));
        assert_eq!(
            Loader::for_server_type(ServerType::NeoForge),
            Some(Loader::NeoForge)
        );
        // Vanilla loads nothing at all.
        assert_eq!(Loader::for_server_type(ServerType::Vanilla), None);
    }

    #[test]
    fn content_directories_match_the_family() {
        assert_eq!(Loader::Paper.content_dir(), "plugins");
        assert_eq!(Loader::Fabric.content_dir(), "mods");
        assert_eq!(Loader::Forge.content_dir(), "mods");
        assert_eq!(Loader::NeoForge.content_dir(), "mods");
    }

    #[test]
    fn accepted_loaders_widen_only_where_that_is_true() {
        assert!(Loader::NeoForge.accepted().contains(&"forge"));
        assert!(!Loader::Forge.accepted().contains(&"neoforge"));
        assert!(Loader::Paper.accepted().contains(&"spigot"));
        assert_eq!(Loader::Fabric.accepted(), vec!["fabric"]);
    }

    #[test]
    fn the_primary_file_is_the_one_marked_primary() {
        let file = |name: &str, primary: bool| SourceFile {
            url: format!("https://cdn.modrinth.com/{name}"),
            file_name: name.to_string(),
            sha1: None,
            sha512: None,
            size: None,
            primary,
        };
        let version = SourceVersion {
            source: SourceId::Modrinth,
            id: "v".into(),
            project_id: "p".into(),
            name: "1.0".into(),
            version_number: "1.0".into(),
            channel: "release".into(),
            published: None,
            game_versions: vec!["1.21.4".into()],
            loaders: vec!["fabric".into()],
            files: vec![file("sources.jar", false), file("mod.jar", true)],
            dependencies: vec![],
        };

        assert_eq!(version.primary_file().unwrap().file_name, "mod.jar");
        assert!(version.is_stable());
    }

    #[test]
    fn a_version_with_no_primary_marker_falls_back_to_the_first_file() {
        let version = SourceVersion {
            source: SourceId::Modrinth,
            id: "v".into(),
            project_id: "p".into(),
            name: "1.0".into(),
            version_number: "1.0".into(),
            channel: "beta".into(),
            published: None,
            game_versions: vec![],
            loaders: vec![],
            files: vec![SourceFile {
                url: "https://cdn.modrinth.com/only.jar".into(),
                file_name: "only.jar".into(),
                sha1: None,
                sha512: None,
                size: None,
                primary: false,
            }],
            dependencies: vec![],
        };
        assert_eq!(version.primary_file().unwrap().file_name, "only.jar");
        assert!(!version.is_stable());
    }
}
