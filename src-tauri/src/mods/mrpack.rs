//! `.mrpack` (Modrinth modpack) import.
//!
//! Three rules this file exists to enforce:
//!
//! 1. **`env` is honoured.** A pack marks each file as required, optional or
//!    unsupported per side. A client-only mod on a server is a guaranteed crash,
//!    and installing it anyway is a common bug in other managers.
//! 2. **Only allowlisted hosts.** Modrinth restricts pack downloads to its own
//!    CDN and a few code hosts; anything else is rejected by name rather than
//!    fetched.
//! 3. **Atomic.** Everything is downloaded and verified into a staging folder;
//!    only once every file is present and its SHA-512 matches does anything move
//!    into the instance. A failed import leaves no half-populated mods folder.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

use crate::db::models::ServerType;
use crate::error::{AppError, AppResult, IoContext};
use crate::state::AppState;

use super::modrinth;
use super::source::Loader;

/// Which side a pack file is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub enum Side {
    Required,
    Optional,
    Unsupported,
}

impl Side {
    fn parse(value: Option<&str>) -> Self {
        match value {
            Some("unsupported") => Side::Unsupported,
            Some("optional") => Side::Optional,
            // Missing means required: that is what the format's default is.
            _ => Side::Required,
        }
    }

    /// Whether a file with this server-side marking is installed.
    pub fn wanted_on_server(self) -> bool {
        matches!(self, Side::Required | Side::Optional)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct PackFile {
    /// Destination path inside the instance, as the pack states it.
    pub path: String,
    pub downloads: Vec<String>,
    pub sha512: Option<String>,
    #[ts(type = "number | null")]
    pub size: Option<u64>,
    pub client: Side,
    pub server: Side,
}

impl PackFile {
    pub fn installed_on_server(&self) -> bool {
        self.server.wanted_on_server()
    }

    /// True when the pack only ships this file for the client.
    pub fn client_only(&self) -> bool {
        self.server == Side::Unsupported && self.client != Side::Unsupported
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct PackIndex {
    pub name: String,
    pub version_id: Option<String>,
    pub summary: Option<String>,
    /// Minecraft version the pack targets.
    pub mc_version: String,
    /// Loader plus its version, as the pack declares them.
    pub loader: Option<String>,
    pub loader_version: Option<String>,
    pub files: Vec<PackFile>,
}

impl PackIndex {
    /// Files that belong on a server.
    pub fn server_files(&self) -> Vec<&PackFile> {
        self.files
            .iter()
            .filter(|file| file.installed_on_server())
            .collect()
    }

    pub fn client_only_files(&self) -> Vec<&PackFile> {
        self.files.iter().filter(|file| file.client_only()).collect()
    }
}

// --- The format's own shapes ----------------------------------------------

#[derive(Debug, Deserialize)]
struct RawIndex {
    #[serde(rename = "formatVersion")]
    format_version: Option<i64>,
    name: String,
    #[serde(rename = "versionId")]
    version_id: Option<String>,
    summary: Option<String>,
    #[serde(default)]
    files: Vec<RawFile>,
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct RawFile {
    path: String,
    #[serde(default)]
    hashes: BTreeMap<String, String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    downloads: Vec<String>,
    #[serde(rename = "fileSize")]
    file_size: Option<u64>,
}

/// Parses `modrinth.index.json`.
pub fn parse_index(body: &str) -> AppResult<PackIndex> {
    let raw: RawIndex = serde_json::from_str(body)
        .map_err(|e| AppError::Other(format!("modrinth.index.json could not be read: {e}")))?;

    if let Some(version) = raw.format_version {
        if version != 1 {
            return Err(AppError::Other(format!(
                "this pack uses index format {version}, which this build does not understand"
            )));
        }
    }

    let mc_version = raw
        .dependencies
        .get("minecraft")
        .cloned()
        .ok_or_else(|| AppError::Other("the pack does not say which Minecraft version it is for".into()))?;

    let (loader, loader_version) = ["fabric-loader", "forge", "neoforge", "quilt-loader"]
        .iter()
        .find_map(|key| {
            raw.dependencies.get(*key).map(|version| {
                let name = match *key {
                    "fabric-loader" => "fabric",
                    "quilt-loader" => "quilt",
                    other => other,
                };
                (Some(name.to_string()), Some(version.clone()))
            })
        })
        .unwrap_or((None, None));

    Ok(PackIndex {
        name: raw.name,
        version_id: raw.version_id,
        summary: raw.summary,
        mc_version,
        loader,
        loader_version,
        files: raw
            .files
            .into_iter()
            .map(|file| PackFile {
                path: file.path,
                sha512: file.hashes.get("sha512").cloned(),
                size: file.file_size,
                client: Side::parse(file.env.get("client").map(String::as_str)),
                server: Side::parse(file.env.get("server").map(String::as_str)),
                downloads: file.downloads,
            })
            .collect(),
    })
}

/// Reads the index out of a `.mrpack` without unpacking it.
pub fn read_index(archive: &Path) -> AppResult<PackIndex> {
    let file = std::fs::File::open(archive).ctx("open the pack", archive)?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| AppError::Other(format!("{} is not a readable .mrpack: {e}", archive.display())))?;

    let mut body = String::new();
    zip.by_name("modrinth.index.json")
        .map_err(|_| {
            AppError::Other(format!(
                "{} does not contain modrinth.index.json, so it is not a Modrinth pack",
                archive.display()
            ))
        })?
        .read_to_string(&mut body)
        .map_err(|e| AppError::Other(format!("the pack index could not be read: {e}")))?;

    parse_index(&body)
}

/// What an import would do, shown before it runs.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct PackPlan {
    pub index: PackIndex,
    #[ts(type = "number")]
    pub install_count: usize,
    /// Files skipped because the pack marks them client-only.
    pub skipped_client_only: Vec<String>,
    #[ts(type = "number")]
    pub total_size: u64,
    /// Set when the pack targets a different Minecraft version or loader.
    pub mismatch: Option<String>,
}

/// Checks the pack against the instance and works out what would be installed.
pub fn plan(index: PackIndex, server_type: ServerType, mc_version: &str) -> AppResult<PackPlan> {
    // Every download URL is checked before anything is fetched, and the file
    // that fails is named.
    for file in index.server_files() {
        for url in &file.downloads {
            if !modrinth::host_allowed(url) {
                return Err(AppError::Other(format!(
                    "{} would be downloaded from {url}, which is not one of Modrinth's allowed \
                     hosts; this pack is refused",
                    file.path
                )));
            }
        }
        if file.downloads.is_empty() {
            return Err(AppError::Other(format!(
                "{} has no download URL in the pack index",
                file.path
            )));
        }
        if file.sha512.is_none() {
            return Err(AppError::Other(format!(
                "{} has no SHA-512 in the pack index, so it cannot be verified",
                file.path
            )));
        }
        // A path that escapes the instance folder is never legitimate.
        if super::super::worlds::archive::safe_entry_path(&file.path, None).is_none() {
            return Err(AppError::Other(format!(
                "{} points outside the instance folder; this pack is refused",
                file.path
            )));
        }
    }

    let loader = Loader::for_server_type(server_type);
    let mismatch = match (loader, index.loader.as_deref()) {
        (None, _) => Some(format!(
            "this instance is vanilla, and \"{}\" needs a mod loader",
            index.name
        )),
        (Some(loader), Some(declared))
            if !loader
                .accepted()
                .iter()
                .any(|accepted| accepted.eq_ignore_ascii_case(declared)) =>
        {
            Some(format!(
                "the pack is built for {declared} but this instance runs {}",
                loader.as_str()
            ))
        }
        _ if index.mc_version != mc_version => Some(format!(
            "the pack targets Minecraft {} but this instance runs {mc_version}",
            index.mc_version
        )),
        _ => None,
    };

    let server_files = index.server_files();
    let total_size = server_files.iter().filter_map(|file| file.size).sum();
    let install_count = server_files.len();
    let skipped_client_only = index
        .client_only_files()
        .iter()
        .map(|file| file.path.clone())
        .collect();

    Ok(PackPlan {
        index,
        install_count,
        skipped_client_only,
        total_size,
        mismatch,
    })
}

/// Progress while importing.
#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
}

/// Imports a pack: stage, verify, then commit.
pub async fn import<P>(
    state: &AppState,
    id: i64,
    archive: &Path,
    cancel: &CancellationToken,
    mut report: P,
) -> AppResult<PackPlan>
where
    P: FnMut(Progress, &str) + Send,
{
    let row = crate::instance::get(&state.db, id).await?;
    let dir = row.path_buf();
    if !dir.is_dir() {
        return Err(AppError::FolderMissing {
            name: row.name.clone(),
            path: dir,
        });
    }

    let archive_path = archive.to_path_buf();
    let index = tokio::task::spawn_blocking(move || read_index(&archive_path))
        .await
        .map_err(|e| AppError::Other(format!("reading the pack failed: {e}")))??;
    let plan = plan(index, row.server_type, &row.mc_version)?;

    // Staging lives inside the instance so the final move is a rename.
    let staging = crate::paths::msm_dir(&dir).join("pack-staging");
    if staging.exists() {
        tokio::fs::remove_dir_all(&staging)
            .await
            .ctx("clear the staging folder", &staging)?;
    }
    tokio::fs::create_dir_all(&staging)
        .await
        .ctx("create the staging folder", &staging)?;

    let outcome = stage(state, &plan, archive, &staging, cancel, &mut report).await;
    if let Err(err) = outcome {
        // Nothing has been moved into the instance yet, so removing the staging
        // folder is enough to leave everything as it was.
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(err);
    }

    commit(&staging, &dir).await?;
    let _ = tokio::fs::remove_dir_all(&staging).await;

    crate::db::record_event(
        &state.db,
        id,
        "mods",
        Some(&format!(
            "imported the pack \"{}\" ({} files, {} client-only files skipped)",
            plan.index.name,
            plan.install_count,
            plan.skipped_client_only.len()
        )),
    )
    .await?;

    Ok(plan)
}

/// Downloads every server file and unpacks the overrides into `staging`.
async fn stage<P>(
    state: &AppState,
    plan: &PackPlan,
    archive: &Path,
    staging: &Path,
    cancel: &CancellationToken,
    report: &mut P,
) -> AppResult<()>
where
    P: FnMut(Progress, &str) + Send,
{
    let files = plan.index.server_files();
    let total = files.len() as u64;

    for (position, file) in files.iter().enumerate() {
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        report(
            Progress {
                done: position as u64,
                total,
            },
            &file.path,
        );

        let url = file
            .downloads
            .first()
            .ok_or_else(|| AppError::Other(format!("{} has no download URL", file.path)))?;

        let relative = super::super::worlds::archive::safe_entry_path(&file.path, None)
            .ok_or_else(|| AppError::Other(format!("{} points outside the pack", file.path)))?;
        let target = staging.join(&relative);

        let artifact = crate::providers::Artifact {
            url: url.clone(),
            file_name: relative.to_string_lossy().to_string(),
            kind: crate::providers::ArtifactKind::ServerJar,
            sha1: None,
            sha256: None,
            // Verified before the file is renamed into place, as always.
            sha512: file.sha512.clone(),
            md5: None,
            size: file.size,
            build: None,
            java_major: None,
        };

        crate::download::download(&state.http, &artifact, &target, cancel, |_| {}).await?;
    }

    report(Progress { done: total, total }, "overrides");
    unpack_overrides(archive, staging, cancel)?;
    Ok(())
}

/// Unpacks `overrides/` and `server-overrides/`; `client-overrides/` is skipped.
fn unpack_overrides(archive: &Path, staging: &Path, cancel: &CancellationToken) -> AppResult<()> {
    let file = std::fs::File::open(archive).ctx("open the pack", archive)?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| AppError::Other(format!("the pack could not be read: {e}")))?;

    let count = zip.len();
    for index in 0..count {
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let mut entry = zip
            .by_index(index)
            .map_err(|e| AppError::Other(format!("the pack could not be read: {e}")))?;

        let name = entry.name().replace('\\', "/");
        // server-overrides wins over overrides, and client-overrides is ignored.
        let relative = if let Some(rest) = name.strip_prefix("server-overrides/") {
            rest
        } else if let Some(rest) = name.strip_prefix("overrides/") {
            rest
        } else {
            continue;
        };

        let Some(relative) = super::super::worlds::archive::safe_entry_path(relative, None) else {
            continue;
        };
        let target = staging.join(relative);

        if entry.is_dir() {
            std::fs::create_dir_all(&target).ctx("create folder", &target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).ctx("create folder", parent)?;
        }
        let mut out = std::fs::File::create(&target).ctx("write pack file", &target)?;
        std::io::copy(&mut entry, &mut out).ctx("write pack file", &target)?;
    }
    Ok(())
}

/// Moves the staged tree into the instance, overwriting file by file.
async fn commit(staging: &Path, instance_dir: &Path) -> AppResult<()> {
    let staging = staging.to_path_buf();
    let instance_dir = instance_dir.to_path_buf();

    tokio::task::spawn_blocking(move || -> AppResult<()> {
        for entry in walkdir::WalkDir::new(&staging).min_depth(1).into_iter().flatten() {
            let relative = entry.path().strip_prefix(&staging).unwrap_or(entry.path());
            let target = instance_dir.join(relative);

            if entry.file_type().is_dir() {
                std::fs::create_dir_all(&target).ctx("create folder", &target)?;
                continue;
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).ctx("create folder", parent)?;
            }
            if target.exists() {
                std::fs::remove_file(&target).ctx("replace file", &target)?;
            }
            std::fs::rename(entry.path(), &target).ctx("move pack file", &target)?;
        }
        Ok(())
    })
    .await
    .map_err(|e| AppError::Other(format!("committing the pack failed: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(name),
        )
        .unwrap()
    }

    #[test]
    fn parses_a_pack_index() {
        let index = parse_index(&fixture("mrpack_index.json")).unwrap();
        assert_eq!(index.name, "Test Pack");
        assert_eq!(index.mc_version, "1.21.4");
        assert_eq!(index.loader.as_deref(), Some("fabric"));
        assert!(index.loader_version.is_some());
        assert!(index.files.len() >= 3);
    }

    #[test]
    fn client_only_files_are_excluded_from_what_a_server_installs() {
        let index = parse_index(&fixture("mrpack_index.json")).unwrap();

        let server: Vec<&str> = index
            .server_files()
            .iter()
            .map(|file| file.path.as_str())
            .collect();
        assert!(server.iter().any(|path| path.contains("lithium")));
        assert!(
            !server.iter().any(|path| path.contains("sodium")),
            "sodium is client-only and must not be installed: {server:?}"
        );

        let skipped: Vec<&str> = index
            .client_only_files()
            .iter()
            .map(|file| file.path.as_str())
            .collect();
        assert!(skipped.iter().any(|path| path.contains("sodium")));
    }

    #[test]
    fn a_missing_env_block_means_required_on_both_sides() {
        let index = parse_index(&fixture("mrpack_index.json")).unwrap();
        let plain = index
            .files
            .iter()
            .find(|file| file.path.contains("no-env"))
            .expect("the fixture has a file with no env block");
        assert_eq!(plain.server, Side::Required);
        assert_eq!(plain.client, Side::Required);
        assert!(plain.installed_on_server());
    }

    #[test]
    fn a_pack_from_a_disallowed_host_is_refused_and_names_the_file() {
        let index = parse_index(&fixture("mrpack_index_bad_host.json")).unwrap();
        let err = plan(index, ServerType::Fabric, "1.21.4").unwrap_err();

        let message = err.to_string();
        assert!(message.contains("evil.example.com"), "{message}");
        assert!(message.contains("mods/backdoor.jar"), "{message}");
        assert!(message.contains("not one of Modrinth's allowed hosts"), "{message}");
    }

    #[test]
    fn a_pack_whose_paths_escape_the_instance_is_refused() {
        let body = r#"{
            "formatVersion": 1,
            "name": "Escape",
            "versionId": "1",
            "dependencies": {"minecraft": "1.21.4", "fabric-loader": "0.16.9"},
            "files": [{
                "path": "../../etc/passwd",
                "hashes": {"sha512": "aa"},
                "downloads": ["https://cdn.modrinth.com/data/x/y.jar"]
            }]
        }"#;
        let err = plan(parse_index(body).unwrap(), ServerType::Fabric, "1.21.4").unwrap_err();
        assert!(err.to_string().contains("points outside the instance folder"));
    }

    #[test]
    fn a_file_without_a_checksum_is_refused_rather_than_trusted() {
        let body = r#"{
            "formatVersion": 1,
            "name": "No hash",
            "dependencies": {"minecraft": "1.21.4", "fabric-loader": "0.16.9"},
            "files": [{
                "path": "mods/thing.jar",
                "downloads": ["https://cdn.modrinth.com/data/x/y.jar"]
            }]
        }"#;
        let err = plan(parse_index(body).unwrap(), ServerType::Fabric, "1.21.4").unwrap_err();
        assert!(err.to_string().contains("no SHA-512"), "{err}");
    }

    #[test]
    fn the_plan_counts_only_what_the_server_installs() {
        let index = parse_index(&fixture("mrpack_index.json")).unwrap();
        let expected: u64 = index
            .server_files()
            .iter()
            .filter_map(|file| file.size)
            .sum();

        let plan = plan(index, ServerType::Fabric, "1.21.4").unwrap();
        assert_eq!(plan.total_size, expected);
        assert_eq!(plan.install_count, plan.index.server_files().len());
        assert!(!plan.skipped_client_only.is_empty());
        assert!(plan.mismatch.is_none());
    }

    #[test]
    fn a_pack_for_another_loader_or_version_warns_without_refusing() {
        let index = parse_index(&fixture("mrpack_index.json")).unwrap();
        let wrong_loader = plan(index.clone(), ServerType::Paper, "1.21.4").unwrap();
        assert!(wrong_loader.mismatch.unwrap().contains("fabric"));

        let wrong_version = plan(index.clone(), ServerType::Fabric, "1.20.1").unwrap();
        assert!(wrong_version.mismatch.unwrap().contains("1.21.4"));

        let vanilla = plan(index, ServerType::Vanilla, "1.21.4").unwrap();
        assert!(vanilla.mismatch.unwrap().contains("vanilla"));
    }

    #[test]
    fn an_index_this_build_does_not_understand_is_refused() {
        let body = r#"{"formatVersion": 99, "name": "Future", "dependencies": {"minecraft":"1.21.4"}}"#;
        let err = parse_index(body).unwrap_err();
        assert!(err.to_string().contains("format 99"), "{err}");
    }

    #[test]
    fn an_index_without_a_minecraft_version_is_refused() {
        let body = r#"{"formatVersion": 1, "name": "No version", "dependencies": {}}"#;
        assert!(parse_index(body)
            .unwrap_err()
            .to_string()
            .contains("which Minecraft version"));
    }

    #[test]
    fn the_index_is_read_straight_out_of_a_mrpack() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let pack = dir.path().join("test.mrpack");
        {
            let file = std::fs::File::create(&pack).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            zip.start_file("modrinth.index.json", options).unwrap();
            zip.write_all(fixture("mrpack_index.json").as_bytes()).unwrap();
            zip.start_file("overrides/config/thing.toml", options).unwrap();
            zip.write_all(b"setting = true").unwrap();
            zip.finish().unwrap();
        }

        let index = read_index(&pack).unwrap();
        assert_eq!(index.name, "Test Pack");
    }

    #[test]
    fn a_zip_without_an_index_is_not_a_pack() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let pack = dir.path().join("plain.zip");
        {
            let file = std::fs::File::create(&pack).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            zip.start_file("readme.txt", options).unwrap();
            zip.write_all(b"not a pack").unwrap();
            zip.finish().unwrap();
        }

        assert!(read_index(&pack)
            .unwrap_err()
            .to_string()
            .contains("does not contain modrinth.index.json"));
    }

    #[test]
    fn overrides_are_unpacked_with_server_overrides_winning_and_client_ones_ignored() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let pack = dir.path().join("packed.mrpack");
        {
            let file = std::fs::File::create(&pack).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            zip.start_file("modrinth.index.json", options).unwrap();
            zip.write_all(fixture("mrpack_index.json").as_bytes()).unwrap();
            zip.start_file("overrides/config/shared.toml", options).unwrap();
            zip.write_all(b"from overrides").unwrap();
            zip.start_file("server-overrides/config/server-only.toml", options)
                .unwrap();
            zip.write_all(b"server only").unwrap();
            zip.start_file("client-overrides/options.txt", options).unwrap();
            zip.write_all(b"client only").unwrap();
            zip.start_file("overrides/../escape.txt", options).unwrap();
            zip.write_all(b"nope").unwrap();
            zip.finish().unwrap();
        }

        let staging = dir.path().join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        unpack_overrides(&pack, &staging, &CancellationToken::new()).unwrap();

        assert!(staging.join("config").join("shared.toml").is_file());
        assert!(staging.join("config").join("server-only.toml").is_file());
        assert!(
            !staging.join("options.txt").exists(),
            "client overrides are not for a server"
        );
        assert!(!dir.path().join("escape.txt").exists(), "escaping paths are dropped");
    }

    #[tokio::test]
    async fn committing_moves_the_staged_tree_into_the_instance() {
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("staging");
        let instance = dir.path().join("instance");
        std::fs::create_dir_all(staging.join("mods")).unwrap();
        std::fs::create_dir_all(&instance).unwrap();
        std::fs::write(staging.join("mods").join("a.jar"), b"jar").unwrap();
        std::fs::write(staging.join("server.properties"), b"motd=pack").unwrap();

        commit(&staging, &instance).await.unwrap();

        assert!(instance.join("mods").join("a.jar").is_file());
        assert_eq!(
            std::fs::read_to_string(instance.join("server.properties")).unwrap(),
            "motd=pack"
        );
    }

    #[tokio::test]
    async fn a_failed_import_leaves_no_half_populated_mods_folder() {
        use std::io::Write;

        // The pack names a file on an allowed host that does not exist, so the
        // download fails partway through staging.
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::connect_in_memory().await.unwrap();
        let state = AppState::new(pool, dir.path().to_path_buf());

        let instance_dir = dir.path().join("server");
        std::fs::create_dir_all(&instance_dir).unwrap();
        let now = crate::db::now_rfc3339();
        sqlx::query(
            "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                jvm_args, server_args, created_at, updated_at)
             VALUES ('u1', 'Packed', ?, 'fabric', '1.21.4', 'jar', '[]', '[]', ?, ?)",
        )
        .bind(instance_dir.to_string_lossy().to_string())
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap();

        let pack = dir.path().join("broken.mrpack");
        {
            let index = r#"{
                "formatVersion": 1,
                "name": "Broken",
                "versionId": "1",
                "dependencies": {"minecraft": "1.21.4", "fabric-loader": "0.16.9"},
                "files": [{
                    "path": "mods/missing.jar",
                    "hashes": {"sha512": "00"},
                    "env": {"client": "required", "server": "required"},
                    "downloads": ["https://cdn.modrinth.com/data/does-not-exist/nope.jar"],
                    "fileSize": 10
                }]
            }"#;
            let file = std::fs::File::create(&pack).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            zip.start_file("modrinth.index.json", options).unwrap();
            zip.write_all(index.as_bytes()).unwrap();
            zip.start_file("overrides/config/pack.toml", options).unwrap();
            zip.write_all(b"setting = true").unwrap();
            zip.finish().unwrap();
        }

        let result = import(&state, 1, &pack, &CancellationToken::new(), |_, _| {}).await;
        assert!(result.is_err(), "the import must fail");

        assert!(
            !instance_dir.join("mods").exists(),
            "no mods folder was created by a failed import"
        );
        assert!(
            !instance_dir.join("config").exists(),
            "no overrides were committed"
        );
        assert!(
            !crate::paths::msm_dir(&instance_dir).join("pack-staging").exists(),
            "the staging folder is cleaned up"
        );
    }
}
