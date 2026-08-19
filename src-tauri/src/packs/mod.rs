//! Browsing modpacks, and turning one into a server.
//!
//! The question this module exists to answer is whether a pack can run as a
//! server at all. Most packs are built for a client, and a launcher that offers
//! every pack and fails during install teaches nobody anything — so a pack is
//! checked first, and the ones that cannot are said so plainly.
//!
//! Two levels of certainty, in order:
//!
//!   1. the source's own answer, where it has one — Modrinth publishes
//!      `server_side` per project;
//!   2. the pack index itself, which is the only real answer: a server-side
//!      loader, and at least one file that is not marked `unsupported` for the
//!      server.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

use crate::db::models::ServerType;
use crate::error::{AppError, AppResult};
use crate::mods::mrpack::{self, PackIndex};
use crate::mods::{AnySource, ModSource, Project, SourceId, SourceVersion};
use crate::state::AppState;

/// How sure this app is that a pack will run as a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub enum ServerSupport {
    /// The pack has a server build: a server loader and files to install.
    Yes,
    /// The source says it has none, or its index has nothing for a server.
    No,
    /// Nothing has been read yet, or the source does not say.
    Unknown,
}

/// What the source says about a project before anything is downloaded.
///
/// Modrinth publishes `server_side`; CurseForge does not, so its packs stay
/// `Unknown` until the index is read.
pub fn declared_support(project: &Project) -> ServerSupport {
    match project.server_side.as_deref() {
        Some("required") | Some("optional") => ServerSupport::Yes,
        Some("unsupported") => ServerSupport::No,
        _ => ServerSupport::Unknown,
    }
}

/// The loaders that can run a server. Anything else is a client-only pack.
pub fn server_type_for_loader(loader: Option<&str>) -> Option<ServerType> {
    match loader?.to_ascii_lowercase().as_str() {
        "fabric" => Some(ServerType::Fabric),
        "forge" => Some(ServerType::Forge),
        "neoforge" => Some(ServerType::NeoForge),
        // Quilt runs servers, but this app cannot install one, so a Quilt pack
        // is not something to offer and then fail on.
        _ => None,
    }
}

/// What reading the index said about a pack.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct PackDetail {
    pub name: String,
    pub mc_version: String,
    pub loader: Option<String>,
    pub loader_version: Option<String>,
    /// The server type this pack becomes, when it can become one.
    pub server_type: Option<ServerType>,
    pub support: ServerSupport,
    /// Why it cannot run as a server, when it cannot.
    pub reason: Option<String>,
    #[ts(type = "number")]
    pub server_files: i64,
    #[ts(type = "number")]
    pub client_only_files: i64,
    #[ts(type = "number")]
    pub total_bytes: i64,
    /// RAM the pack itself asks for, when its description says.
    #[ts(type = "number | null")]
    pub published_ram_mb: Option<i64>,
    /// What this app would suggest otherwise, from the size of the pack.
    #[ts(type = "number")]
    pub suggested_ram_mb: i64,
}

/// Reads a downloaded pack and decides whether it can be a server.
pub fn inspect(index: PackIndex, description: Option<&str>) -> PackDetail {
    let server_type = server_type_for_loader(index.loader.as_deref());
    let server_files = index.server_files().len() as i64;
    let client_only = index.client_only_files().len() as i64;
    let total_bytes: i64 = index
        .server_files()
        .iter()
        .filter_map(|file| file.size)
        .sum::<u64>() as i64;

    let (support, reason) = match (server_type, server_files) {
        (None, _) => (
            ServerSupport::No,
            Some(match index.loader.as_deref() {
                Some(loader) => format!(
                    "This pack is built for {loader}, which this app cannot run as a server."
                ),
                None => "This pack does not name a mod loader, so there is no server to run."
                    .to_string(),
            }),
        ),
        (Some(_), 0) => (
            ServerSupport::No,
            Some(
                "Every file in this pack is marked client-only, so there is no server build."
                    .to_string(),
            ),
        ),
        (Some(_), _) => (ServerSupport::Yes, None),
    };

    PackDetail {
        published_ram_mb: description.and_then(published_ram_mb),
        suggested_ram_mb: suggested_ram_mb(server_files),
        name: index.name,
        mc_version: index.mc_version,
        loader: index.loader,
        loader_version: index.loader_version,
        server_type,
        support,
        reason,
        server_files,
        client_only_files: client_only,
        total_bytes,
    }
}

/// RAM the pack's own text asks for, in MB.
///
/// Neither pack format has a field for it, so the only place it exists is the
/// description — and packs do say, because it matters. Read rather than
/// guessed, and clearly separated from this app's own suggestion.
pub fn published_ram_mb(description: &str) -> Option<i64> {
    let lowered = description.to_ascii_lowercase();
    let marker = ["recommended ram", "recommended memory", "ram:", "memory:"]
        .iter()
        .find_map(|marker| lowered.find(marker).map(|at| at + marker.len()))?;

    let tail: String = lowered[marker..].chars().take(24).collect();
    let digits: String = tail
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let amount: f64 = digits.parse().ok()?;
    if amount <= 0.0 {
        return None;
    }

    let unit_at = tail.find(&digits)? + digits.len();
    let unit: String = tail[unit_at..].chars().take(3).collect();
    let mb = if unit.trim_start().starts_with('g') {
        amount * 1024.0
    } else if unit.trim_start().starts_with('m') {
        amount
    } else if amount <= 64.0 {
        // A bare number that small is gigabytes; nobody asks for 8 MB.
        amount * 1024.0
    } else {
        amount
    };

    let rounded = mb.round() as i64;
    (512..=65_536).contains(&rounded).then_some(rounded)
}

/// What to pre-fill when the pack does not say: a floor of 4 GB, plus room for
/// what it installs, capped at something a normal machine can give.
pub fn suggested_ram_mb(server_files: i64) -> i64 {
    let extra = (server_files / 50) * 1024;
    (4096 + extra).clamp(4096, 12_288)
}

/// Where a downloaded pack is cached.
pub fn pack_cache_path(data_dir: &Path, version: &SourceVersion) -> PathBuf {
    let file = version
        .primary_file()
        .map(|file| file.file_name.clone())
        .unwrap_or_else(|| format!("{}.mrpack", version.id.replace('/', "-")));
    data_dir.join("cache").join("packs").join(file)
}

/// Downloads the pack file for a version, verified like anything else.
pub async fn fetch_pack(
    state: &AppState,
    version: &SourceVersion,
    cancel: &CancellationToken,
) -> AppResult<PathBuf> {
    let file = version.primary_file().ok_or_else(|| {
        AppError::Other(
            "this pack has no downloadable file — the author may not allow it through the API"
                .into(),
        )
    })?;
    if !crate::mods::download_host_allowed(version.source, &file.url) {
        return Err(AppError::Other(format!(
            "{} would be downloaded from {}, which is not an allowed host",
            file.file_name, file.url
        )));
    }

    let artifact = crate::providers::Artifact {
        url: file.url.clone(),
        file_name: file.file_name.clone(),
        kind: crate::providers::ArtifactKind::ServerJar,
        sha1: file.sha1.clone(),
        sha256: None,
        sha512: file.sha512.clone(),
        md5: None,
        size: file.size,
        build: Some(version.version_number.clone()),
        java_major: None,
    };

    let target = pack_cache_path(&state.data_dir, version);
    crate::download::download(&state.http, &artifact, &target, cancel, |_| {}).await?;
    Ok(target)
}

/// Reads a pack's index without installing anything.
pub async fn examine(
    state: &AppState,
    source: SourceId,
    project_id: &str,
    version_id: &str,
    cancel: &CancellationToken,
) -> AppResult<PackDetail> {
    let client = AnySource::build(state, source).await?;
    let version = client.version(version_id).await?;
    let project = client.project(project_id).await.ok();

    // A source that already says no is taken at its word: downloading a pack to
    // confirm what it told us would be rude to it and slow for the user.
    if let Some(project) = &project {
        if declared_support(project) == ServerSupport::No {
            return Ok(PackDetail {
                name: project.title.clone(),
                mc_version: version
                    .game_versions
                    .first()
                    .cloned()
                    .unwrap_or_default(),
                loader: version.loaders.first().cloned(),
                loader_version: None,
                server_type: None,
                support: ServerSupport::No,
                reason: Some(
                    "The author marks this pack as unsupported on servers, so it has no server \
                     build."
                        .into(),
                ),
                server_files: 0,
                client_only_files: 0,
                total_bytes: 0,
                published_ram_mb: None,
                suggested_ram_mb: suggested_ram_mb(0),
            });
        }
    }

    let archive = fetch_pack(state, &version, cancel).await?;
    let index = tokio::task::spawn_blocking(move || mrpack::read_index(&archive))
        .await
        .map_err(|e| AppError::internal("reading the pack", e))??;

    let description = project
        .as_ref()
        .and_then(|project| project.body.clone().or_else(|| Some(project.description.clone())));
    Ok(inspect(index, description.as_deref()))
}

/// Everything a pack install needs from the user.
#[derive(Debug, Clone, serde::Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct InstallPackInput {
    pub source: SourceId,
    pub project_id: String,
    pub version_id: String,
    /// Name for the new instance.
    pub name: String,
    /// Absolute folder for it.
    pub path: String,
    #[ts(type = "number | null")]
    pub max_ram_mb: Option<i64>,
}

/// Turns a pack into a server.
///
/// The order matters: the instance row and folder first, then the loader's own
/// server, then the pack's files through the Phase 5 importer — which honours
/// `env`, verifies every hash and stages before it commits. A pack that cannot
/// run on a server is refused here, not discovered half way through.
pub async fn install<P>(
    state: &AppState,
    input: InstallPackInput,
    cancel: &CancellationToken,
    mut report: P,
) -> AppResult<i64>
where
    P: FnMut(&str, u64, Option<u64>) + Send,
{
    report("Reading the pack", 0, None);
    let detail = examine(state, input.source, &input.project_id, &input.version_id, cancel).await?;

    let Some(server_type) = detail.server_type.filter(|_| detail.support == ServerSupport::Yes)
    else {
        return Err(AppError::Other(
            detail
                .reason
                .unwrap_or_else(|| "This pack has no server build.".to_string()),
        ));
    };

    let instance = crate::instance::crud::create(
        state,
        crate::instance::CreateInstanceInput {
            name: input.name,
            path: input.path,
            server_type,
            mc_version: detail.mc_version.clone(),
            loader_version: detail.loader_version.clone(),
            min_ram_mb: None,
            max_ram_mb: Some(
                input
                    .max_ram_mb
                    .or(detail.published_ram_mb)
                    .unwrap_or(detail.suggested_ram_mb),
            ),
            notes: Some(format!("Installed from the {} modpack", detail.name)),
            color: None,
        },
    )
    .await?;

    // From here anything that fails leaves a created instance behind, which is
    // recoverable — the folder is there, and the user can install into it or
    // delete it — where a half-applied pack inside a working server would not
    // be. The importer stages its own work regardless.
    report("Installing the server", 0, None);
    crate::instance::install::install(
        state,
        &state.http,
        &instance,
        &detail.mc_version,
        detail.loader_version.as_deref(),
        cancel,
        |_, done, total, message| report(&message, done, total),
    )
    .await?;

    report("Installing the pack's mods", 0, None);
    let archive = {
        let client = AnySource::build(state, input.source).await?;
        let version = client.version(&input.version_id).await?;
        fetch_pack(state, &version, cancel).await?
    };
    mrpack::import(state, instance.id, &archive, cancel, |progress, message| {
        report(message, progress.done, Some(progress.total));
    })
    .await?;

    Ok(instance.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::mrpack::parse_index;

    fn index_json(loader: &str, files: &str) -> String {
        format!(
            r#"{{
                "formatVersion": 1,
                "game": "minecraft",
                "versionId": "1.0.0",
                "name": "Test Pack",
                "summary": "a pack",
                "files": [{files}],
                "dependencies": {{ "minecraft": "1.21.4", "{loader}": "0.16.9" }}
            }}"#
        )
    }

    const SERVER_FILE: &str = r#"{
        "path": "mods/sodium.jar",
        "hashes": { "sha512": "aa" },
        "env": { "client": "required", "server": "required" },
        "downloads": ["https://cdn.modrinth.com/data/x/sodium.jar"],
        "fileSize": 1048576
    }"#;

    const CLIENT_ONLY_FILE: &str = r#"{
        "path": "mods/iris.jar",
        "hashes": { "sha512": "bb" },
        "env": { "client": "required", "server": "unsupported" },
        "downloads": ["https://cdn.modrinth.com/data/y/iris.jar"],
        "fileSize": 2097152
    }"#;

    #[test]
    fn a_pack_with_a_server_loader_and_server_files_can_be_installed() {
        let index = parse_index(&index_json("fabric-loader", SERVER_FILE)).unwrap();
        let detail = inspect(index, None);

        assert_eq!(detail.support, ServerSupport::Yes);
        assert_eq!(detail.server_type, Some(ServerType::Fabric));
        assert_eq!(detail.server_files, 1);
        assert_eq!(detail.mc_version, "1.21.4");
        assert!(detail.reason.is_none());
        assert_eq!(detail.total_bytes, 1_048_576);
    }

    #[test]
    fn a_pack_whose_files_are_all_client_only_says_it_has_no_server_build() {
        let index = parse_index(&index_json("fabric-loader", CLIENT_ONLY_FILE)).unwrap();
        let detail = inspect(index, None);

        assert_eq!(detail.support, ServerSupport::No);
        assert_eq!(detail.client_only_files, 1);
        assert_eq!(detail.server_files, 0);
        let reason = detail.reason.expect("it says why");
        assert!(reason.contains("client-only"), "{reason}");
        assert!(reason.contains("no server build"), "{reason}");
    }

    #[test]
    fn a_pack_for_a_loader_this_app_cannot_run_is_refused_by_name() {
        let index = parse_index(&index_json("quilt-loader", SERVER_FILE)).unwrap();
        let detail = inspect(index, None);

        assert_eq!(detail.support, ServerSupport::No);
        assert_eq!(detail.server_type, None);
        assert!(detail.reason.unwrap().contains("quilt"), "the loader is named");
    }

    #[test]
    fn the_sources_own_answer_is_used_where_it_has_one() {
        let project = |side: Option<&str>| Project {
            source: SourceId::Modrinth,
            id: "p".into(),
            slug: "p".into(),
            title: "Pack".into(),
            description: String::new(),
            author: None,
            downloads: None,
            icon_url: None,
            page_url: None,
            categories: vec![],
            loaders: vec![],
            updated: None,
            content_type: None,
            license: None,
            source_url: None,
            issues_url: None,
            wiki_url: None,
            body: None,
            server_side: side.map(str::to_string),
            downloadable: true,
        };

        assert_eq!(declared_support(&project(Some("required"))), ServerSupport::Yes);
        assert_eq!(declared_support(&project(Some("optional"))), ServerSupport::Yes);
        assert_eq!(declared_support(&project(Some("unsupported"))), ServerSupport::No);
        // CurseForge says nothing, so the index is the only answer.
        assert_eq!(declared_support(&project(None)), ServerSupport::Unknown);
        assert_eq!(declared_support(&project(Some("unknown"))), ServerSupport::Unknown);
    }

    #[test]
    fn the_ram_a_pack_asks_for_is_read_from_what_it_says() {
        assert_eq!(published_ram_mb("Recommended RAM: 6GB"), Some(6144));
        assert_eq!(published_ram_mb("recommended memory 8 GB for servers"), Some(8192));
        assert_eq!(published_ram_mb("RAM: 4096MB"), Some(4096));
        // A bare number that small can only be gigabytes.
        assert_eq!(published_ram_mb("Recommended RAM: 10"), Some(10240));

        // Nothing to read, and nothing absurd taken seriously.
        assert_eq!(published_ram_mb("a lovely kitchen-sink pack"), None);
        assert_eq!(published_ram_mb("Recommended RAM: 0GB"), None);
        assert_eq!(published_ram_mb("RAM: 900GB"), None);
    }

    #[test]
    fn the_suggestion_starts_at_four_gigabytes_and_grows_with_the_pack() {
        assert_eq!(suggested_ram_mb(0), 4096);
        assert_eq!(suggested_ram_mb(40), 4096);
        assert_eq!(suggested_ram_mb(100), 6144);
        assert_eq!(suggested_ram_mb(250), 9216);
        // And never asks for more than a normal machine has.
        assert_eq!(suggested_ram_mb(5_000), 12_288);
    }

    #[test]
    fn a_pack_is_cached_under_its_own_file_name() {
        let version = SourceVersion {
            source: SourceId::Modrinth,
            id: "v1".into(),
            project_id: "p".into(),
            name: "1.0".into(),
            version_number: "1.0".into(),
            channel: "release".into(),
            published: None,
            game_versions: vec!["1.21.4".into()],
            loaders: vec!["fabric".into()],
            files: vec![crate::mods::source::SourceFile {
                url: "https://cdn.modrinth.com/data/p/versions/v1/pack.mrpack".into(),
                file_name: "pack.mrpack".into(),
                sha1: None,
                sha512: Some("aa".into()),
                size: Some(1024),
                primary: true,
            }],
            dependencies: vec![],
        };

        let path = pack_cache_path(Path::new("/data"), &version);
        assert!(path.ends_with("pack.mrpack"));
        assert!(path.starts_with(Path::new("/data").join("cache").join("packs")));
    }
}
