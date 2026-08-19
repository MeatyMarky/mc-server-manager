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

/// What the browser is showing: mods, plugins, packs, and the client-side kinds
/// a server has no use for.
///
/// Each source calls these something different — a Modrinth "project type", a
/// CurseForge "class id" — and neither name appears above this boundary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub enum ContentType {
    #[default]
    Mod,
    Plugin,
    Modpack,
    DataPack,
    ResourcePack,
    Shader,
}

impl ContentType {
    /// True for content a server never loads. Offered anyway, clearly marked,
    /// because people do browse for what they will hand to their players.
    pub fn is_client_only(self) -> bool {
        matches!(self, ContentType::ResourcePack | ContentType::Shader)
    }

    /// What is worth offering for a server of this type.
    ///
    /// A Paper server loads plugins and never mods; a Fabric server the other
    /// way round. Vanilla loads neither, but still takes data packs.
    pub fn for_server_type(server_type: ServerType) -> Vec<Self> {
        let mut kinds = match Loader::for_server_type(server_type) {
            Some(Loader::Paper) => vec![ContentType::Plugin],
            Some(_) => vec![ContentType::Mod, ContentType::Modpack],
            None => vec![],
        };
        kinds.push(ContentType::DataPack);
        kinds.push(ContentType::ResourcePack);
        kinds.push(ContentType::Shader);
        kinds
    }

    /// Whether this instance could install it at all.
    pub fn installable_on(self, server_type: ServerType) -> bool {
        !self.is_client_only()
            && Self::for_server_type(server_type).contains(&self)
            && self != ContentType::Modpack
    }
}

/// How the results are ordered. Mapped onto each source's own parameter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub enum SortBy {
    #[default]
    Relevance,
    Popularity,
    Downloads,
    RecentlyUpdated,
    Newest,
}

/// One filterable category, as the source publishes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct Category {
    /// The value to send back in a search.
    pub id: String,
    /// What to show in the dropdown.
    pub name: String,
}

/// One page of results, with enough to drive pagination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct SearchPage {
    pub projects: Vec<Project>,
    /// Total matches the source reports, when it reports one.
    #[ts(type = "number | null")]
    pub total: Option<i64>,
    #[ts(type = "number")]
    pub offset: u32,
    #[ts(type = "number")]
    pub limit: u32,
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
    /// When the project last published something, as the source reports it.
    pub updated: Option<String>,
    /// What kind of content this is, when the source says.
    pub content_type: Option<ContentType>,
    /// False when the source will not serve the files through its API.
    ///
    /// CurseForge lets an author forbid third-party downloads, and a browser
    /// that only fails at install time teaches nobody anything — so the card
    /// says so and links to the page instead.
    pub downloadable: bool,
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
    #[serde(default)]
    pub sort: SortBy,
    /// Category ids from the source's own list; empty means any.
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub content_type: ContentType,
}

impl SearchQuery {
    pub fn page_size(&self) -> u32 {
        self.limit.unwrap_or(20).clamp(1, 100)
    }

    pub fn page_offset(&self) -> u32 {
        self.offset.unwrap_or(0)
    }
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

    fn search(&self, query: &SearchQuery) -> impl Future<Output = AppResult<SearchPage>> + Send;

    /// Categories this source offers for a kind of content, already narrowed to
    /// what is worth showing for a server.
    fn categories(
        &self,
        content_type: ContentType,
    ) -> impl Future<Output = AppResult<Vec<Category>>> + Send;

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

    #[test]
    fn content_types_follow_the_server_type() {
        // A Paper server loads plugins, never mods.
        let paper = ContentType::for_server_type(ServerType::Paper);
        assert!(paper.contains(&ContentType::Plugin));
        assert!(!paper.contains(&ContentType::Mod));

        // A Fabric server the other way round, and it can take a pack.
        let fabric = ContentType::for_server_type(ServerType::Fabric);
        assert!(fabric.contains(&ContentType::Mod));
        assert!(fabric.contains(&ContentType::Modpack));
        assert!(!fabric.contains(&ContentType::Plugin));

        // Vanilla loads neither, but data packs are still worth browsing.
        let vanilla = ContentType::for_server_type(ServerType::Vanilla);
        assert!(!vanilla.contains(&ContentType::Mod));
        assert!(!vanilla.contains(&ContentType::Plugin));
        assert!(vanilla.contains(&ContentType::DataPack));
    }

    #[test]
    fn client_only_content_is_offered_but_marked() {
        assert!(ContentType::ResourcePack.is_client_only());
        assert!(ContentType::Shader.is_client_only());
        assert!(!ContentType::Mod.is_client_only());
        assert!(!ContentType::DataPack.is_client_only());

        // Every server type can browse them; none can install them.
        for server_type in [ServerType::Paper, ServerType::Fabric, ServerType::Vanilla] {
            let kinds = ContentType::for_server_type(server_type);
            assert!(kinds.contains(&ContentType::Shader), "{server_type:?}");
            assert!(!ContentType::Shader.installable_on(server_type));
            assert!(!ContentType::ResourcePack.installable_on(server_type));
        }

        // A pack is browsed here and installed by creating an instance, not by
        // dropping files into an existing one.
        assert!(!ContentType::Modpack.installable_on(ServerType::Fabric));
        assert!(ContentType::Mod.installable_on(ServerType::Fabric));
        assert!(ContentType::Plugin.installable_on(ServerType::Paper));
        assert!(!ContentType::Mod.installable_on(ServerType::Paper));
    }

    #[test]
    fn a_query_has_a_sane_page_size_whatever_it_is_asked_for() {
        let query = SearchQuery::default();
        assert_eq!(query.page_size(), 20);
        assert_eq!(query.page_offset(), 0);
        assert_eq!(query.sort, SortBy::Relevance);
        assert_eq!(query.content_type, ContentType::Mod);

        let huge = SearchQuery {
            limit: Some(5_000),
            offset: Some(40),
            ..SearchQuery::default()
        };
        assert_eq!(huge.page_size(), 100, "a source will not serve more");
        assert_eq!(huge.page_offset(), 40);

        assert_eq!(
            SearchQuery {
                limit: Some(0),
                ..SearchQuery::default()
            }
            .page_size(),
            1
        );
    }
}
