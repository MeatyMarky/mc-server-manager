//! Mods and plugins: what is installed, and how it gets there.
//!
//! The install target is decided by the *server type*, never by looking at the
//! jar: Paper and Purpur load `plugins/`, the mod loaders load `mods/`, and
//! vanilla loads neither — installing into a vanilla instance is refused with a
//! sentence saying why rather than silently creating a folder nothing reads.

pub mod curseforge;
pub mod icons;
pub mod jarmeta;
pub mod modrinth;
pub mod mrpack;
pub mod ratelimit;
pub mod resolve;
pub mod source;

use std::path::{Path, PathBuf};

use serde::Serialize;
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

use crate::db::models::{Instance, ServerType};
use crate::db::{now_rfc3339, record_event};
use crate::error::{AppError, AppResult, IoContext};
use crate::instance;
use crate::state::AppState;

pub use jarmeta::{JarMetadata, Mismatch};
pub use resolve::{InstallPlan, Installed, PlannedMod};
/// Whichever source the caller asked for, as one value.
///
/// The trait uses `impl Future`, so it cannot be made into a trait object; an
/// enum that delegates keeps every call site written once and keeps the two
/// implementations completely separate, which is the point of the boundary.
pub enum AnySource {
    Modrinth(modrinth::Modrinth),
    CurseForge(curseforge::CurseForge),
}

impl AnySource {
    /// Builds the source a caller named, reading whatever configuration it
    /// needs. A CurseForge without a key is a state with an explanation, not a
    /// mysterious failure.
    pub async fn build(state: &crate::state::AppState, id: SourceId) -> AppResult<Self> {
        match id {
            SourceId::CurseForge => {
                let key = crate::db::setting_get(&state.db, curseforge::KEY_SETTING)
                    .await?
                    .map(|key| key.trim().to_string())
                    .filter(|key| !key.is_empty())
                    .ok_or_else(|| {
                        AppError::Other(
                            "CurseForge needs an API key. Add one in Settings — it is free, and \
                             CurseForge requires every application to use its own."
                                .to_string(),
                        )
                    })?;
                Ok(AnySource::CurseForge(curseforge::CurseForge::new(
                    key,
                    state.rate_limiter.clone(),
                )?))
            }
            // A local jar has no API behind it; searching falls back to the one
            // source that can answer.
            _ => Ok(AnySource::Modrinth(modrinth::Modrinth::new(
                state.rate_limiter.clone(),
            )?)),
        }
    }
}

impl ModSource for AnySource {
    fn id(&self) -> SourceId {
        match self {
            AnySource::Modrinth(source) => source.id(),
            AnySource::CurseForge(source) => source.id(),
        }
    }

    async fn search(&self, query: &SearchQuery) -> AppResult<SearchPage> {
        match self {
            AnySource::Modrinth(source) => source.search(query).await,
            AnySource::CurseForge(source) => source.search(query).await,
        }
    }

    async fn categories(&self, content_type: ContentType) -> AppResult<Vec<Category>> {
        match self {
            AnySource::Modrinth(source) => source.categories(content_type).await,
            AnySource::CurseForge(source) => source.categories(content_type).await,
        }
    }

    async fn project(&self, project_id: &str) -> AppResult<Project> {
        match self {
            AnySource::Modrinth(source) => source.project(project_id).await,
            AnySource::CurseForge(source) => source.project(project_id).await,
        }
    }

    async fn versions(
        &self,
        project_id: &str,
        filter: &VersionFilter,
    ) -> AppResult<Vec<SourceVersion>> {
        match self {
            AnySource::Modrinth(source) => source.versions(project_id, filter).await,
            AnySource::CurseForge(source) => source.versions(project_id, filter).await,
        }
    }

    async fn version(&self, version_id: &str) -> AppResult<SourceVersion> {
        match self {
            AnySource::Modrinth(source) => source.version(version_id).await,
            AnySource::CurseForge(source) => source.version(version_id).await,
        }
    }
}

pub use source::{
    Category, ContentType, Loader, ModSource, Project, SearchPage, SearchQuery, SortBy, SourceId,
    SourceVersion, VersionFilter,
};

/// Suffix a disabled jar carries. The server ignores it; we rename rather than
/// move so the file stays next to its siblings.
pub const DISABLED_SUFFIX: &str = ".disabled";

#[derive(Debug, Clone, Serialize, sqlx::FromRow, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct InstalledMod {
    #[ts(type = "number")]
    pub id: i64,
    pub file_name: String,
    pub display_name: Option<String>,
    pub version: Option<String>,
    pub loader: Option<String>,
    pub mc_version: Option<String>,
    pub source: String,
    pub project_id: Option<String>,
    pub version_id: Option<String>,
    pub page_url: Option<String>,
    #[ts(type = "number | null")]
    pub size_bytes: Option<i64>,
    pub enabled: bool,
    pub pinned: bool,
    /// Newest version id seen by an update check, when it differs.
    pub update_version_id: Option<String>,
    pub installed_at: String,
}

/// A jar in the content folder, with whatever the database and the jar itself
/// know about it.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct ModView {
    pub file_name: String,
    pub enabled: bool,
    #[ts(type = "number")]
    pub size_bytes: u64,
    /// The row from `mods`, when this app installed it.
    pub tracked: Option<InstalledMod>,
    /// What the jar declares about itself.
    pub metadata: Option<JarMetadata>,
    /// Set when the jar's declarations do not match the instance.
    pub mismatch: Option<Mismatch>,
    /// Projects that depend on this one, for the uninstall warning.
    pub required_by: Vec<String>,
}

/// Everything the mods tab needs in one call.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct ModsView {
    /// `mods` or `plugins`, decided by the server type.
    pub content_dir: Option<String>,
    pub loader: Option<Loader>,
    pub mc_version: String,
    pub mods: Vec<ModView>,
    /// Set when this instance cannot load mods at all.
    pub unsupported: Option<String>,
}

/// The folder jars go into, or an error explaining why there is none.
pub fn content_dir(instance: &Instance) -> AppResult<PathBuf> {
    let Some(loader) = Loader::for_server_type(instance.server_type) else {
        return Err(AppError::Other(format!(
            "\"{}\" is a vanilla server, which loads no mods or plugins. \
             Install Fabric, Forge or NeoForge for mods, or Paper or Purpur for plugins.",
            instance.name
        )));
    };
    Ok(instance.path_buf().join(loader.content_dir()))
}

/// True when the file name is a jar this app would manage.
pub fn is_jar(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    lower.ends_with(".jar") || lower.ends_with(".jar.disabled")
}

/// The name without the disabled suffix, which is the identity used everywhere.
pub fn base_name(file_name: &str) -> String {
    file_name
        .strip_suffix(DISABLED_SUFFIX)
        .unwrap_or(file_name)
        .to_string()
}

pub fn is_enabled(file_name: &str) -> bool {
    !file_name.ends_with(DISABLED_SUFFIX)
}

/// The name a jar should have for the wanted state.
pub fn name_for(file_name: &str, enabled: bool) -> String {
    let base = base_name(file_name);
    if enabled {
        base
    } else {
        format!("{base}{DISABLED_SUFFIX}")
    }
}

/// Lists the content folder, joined with what the database knows.
pub async fn list(state: &AppState, id: i64) -> AppResult<ModsView> {
    let row = instance::get(&state.db, id).await?;
    let dir = row.path_buf();
    if !dir.is_dir() {
        return Err(AppError::FolderMissing {
            name: row.name.clone(),
            path: dir,
        });
    }

    let Some(loader) = Loader::for_server_type(row.server_type) else {
        return Ok(ModsView {
            content_dir: None,
            loader: None,
            mc_version: row.mc_version.clone(),
            mods: Vec::new(),
            unsupported: Some(format!(
                "\"{}\" is a vanilla server, which loads no mods or plugins. \
                 Reinstall it as Fabric, Forge or NeoForge for mods, or Paper or Purpur for plugins.",
                row.name
            )),
        });
    };

    let folder = content_dir(&row)?;
    let tracked = sqlx::query_as::<_, InstalledMod>(
        "SELECT id, file_name, display_name, version, loader, mc_version, source, project_id,
                version_id, page_url, size_bytes, enabled, pinned, update_version_id, installed_at
         FROM mods WHERE instance_id = ?",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;

    let dependents = dependents_map(state, id).await?;
    let mc_version = row.mc_version.clone();

    let mods = tokio::task::spawn_blocking(move || {
        scan(&folder, &tracked, &dependents, loader, &mc_version)
    })
    .await
    .map_err(|e| AppError::internal("scanning the content folder", e))?;

    Ok(ModsView {
        content_dir: Some(loader.content_dir().to_string()),
        loader: Some(loader),
        mc_version: row.mc_version,
        mods,
        unsupported: None,
    })
}

/// Reads the folder and every jar's metadata. Blocking.
fn scan(
    folder: &Path,
    tracked: &[InstalledMod],
    dependents: &std::collections::BTreeMap<String, Vec<String>>,
    loader: Loader,
    mc_version: &str,
) -> Vec<ModView> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };

    let mut mods: Vec<ModView> = entries
        .flatten()
        .filter(|entry| entry.path().is_file())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(is_jar)
                .unwrap_or(false)
        })
        .map(|path| {
            let file_name = path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            let base = base_name(&file_name);

            let metadata = jarmeta::read_jar(&path).ok().flatten();
            let mismatch = metadata
                .as_ref()
                .map(|metadata| jarmeta::check(metadata, loader, mc_version))
                .filter(|mismatch| !mismatch.is_empty());

            let tracked_row = tracked
                .iter()
                .find(|row| base_name(&row.file_name) == base)
                .cloned();

            ModView {
                enabled: is_enabled(&file_name),
                size_bytes: std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0),
                required_by: tracked_row
                    .as_ref()
                    .and_then(|row| row.project_id.clone())
                    .and_then(|project| dependents.get(&project).cloned())
                    .unwrap_or_default(),
                tracked: tracked_row,
                metadata,
                mismatch,
                file_name,
            }
        })
        .collect();

    mods.sort_by_key(|entry| entry.file_name.to_lowercase());
    mods
}

/// project id -> titles of installed mods that require it.
async fn dependents_map(
    state: &AppState,
    id: i64,
) -> AppResult<std::collections::BTreeMap<String, Vec<String>>> {
    let rows: Vec<(String, Option<String>, String)> = sqlx::query_as(
        "SELECT d.dep_project_id, m.display_name, m.file_name
         FROM mod_dependencies d
         JOIN mods m ON m.id = d.mod_id
         WHERE m.instance_id = ? AND d.required = 1",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;

    let mut map: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for (dependency, name, file_name) in rows {
        map.entry(dependency)
            .or_default()
            .push(name.unwrap_or(file_name));
    }
    Ok(map)
}

/// What is installed, for dependency resolution.
pub async fn installed(state: &AppState, id: i64) -> AppResult<Installed> {
    let rows: Vec<(Option<String>, Option<String>, String)> =
        sqlx::query_as("SELECT project_id, version_id, file_name FROM mods WHERE instance_id = ?")
            .bind(id)
            .fetch_all(&state.db)
            .await?;

    let mut installed = Installed::default();
    for (project_id, version_id, file_name) in rows {
        if let (Some(project), Some(version)) = (project_id, version_id) {
            installed.by_project.insert(project, (version, file_name));
        }
    }
    Ok(installed)
}

/// Enables or disables a jar by renaming it.
pub async fn set_enabled(state: &AppState, id: i64, file_name: &str, enabled: bool) -> AppResult<()> {
    let row = instance::get(&state.db, id).await?;
    let folder = content_dir(&row)?;
    let current = folder.join(file_name);
    if !current.is_file() {
        return Err(AppError::Other(format!("{file_name} is not in this instance")));
    }

    let target = folder.join(name_for(file_name, enabled));
    if current != target {
        tokio::fs::rename(&current, &target)
            .await
            .ctx("rename jar", &target)?;
    }

    sqlx::query(
        "UPDATE mods SET enabled = ?, file_name = ?, updated_at = ?
         WHERE instance_id = ? AND file_name IN (?, ?)",
    )
    .bind(enabled)
    .bind(base_name(file_name))
    .bind(now_rfc3339())
    .bind(id)
    .bind(base_name(file_name))
    .bind(format!("{}{DISABLED_SUFFIX}", base_name(file_name)))
    .execute(&state.db)
    .await?;

    Ok(())
}

/// Pins a mod so update checks leave it alone.
pub async fn set_pinned(state: &AppState, id: i64, file_name: &str, pinned: bool) -> AppResult<()> {
    sqlx::query("UPDATE mods SET pinned = ?, updated_at = ? WHERE instance_id = ? AND file_name = ?")
        .bind(pinned)
        .bind(now_rfc3339())
        .bind(id)
        .bind(base_name(file_name))
        .execute(&state.db)
        .await?;
    Ok(())
}

/// Removes a jar and forgets it. The caller has already shown any dependency
/// warning; this reports what depended on it so the command can pass it on.
pub async fn uninstall(state: &AppState, id: i64, file_name: &str) -> AppResult<Vec<String>> {
    let row = instance::get(&state.db, id).await?;
    let folder = content_dir(&row)?;
    let base = base_name(file_name);

    let dependents = dependents_map(state, id).await?;
    let project: Option<String> =
        sqlx::query_scalar("SELECT project_id FROM mods WHERE instance_id = ? AND file_name = ?")
            .bind(id)
            .bind(&base)
            .fetch_optional(&state.db)
            .await?
            .flatten();

    let warnings = project
        .as_ref()
        .and_then(|project| dependents.get(project).cloned())
        .unwrap_or_default();

    for candidate in [base.clone(), format!("{base}{DISABLED_SUFFIX}")] {
        let path = folder.join(&candidate);
        if path.is_file() {
            tokio::fs::remove_file(&path).await.ctx("delete jar", &path)?;
        }
    }

    sqlx::query("DELETE FROM mods WHERE instance_id = ? AND file_name = ?")
        .bind(id)
        .bind(&base)
        .execute(&state.db)
        .await?;

    record_event(&state.db, id, "mods", Some(&format!("removed {base}"))).await?;
    Ok(warnings)
}

/// Copies a jar the user supplied into the content folder.
pub async fn install_local(state: &AppState, id: i64, jar: &Path) -> AppResult<ModView> {
    let row = instance::get(&state.db, id).await?;
    let folder = content_dir(&row)?;
    tokio::fs::create_dir_all(&folder)
        .await
        .ctx("create content folder", &folder)?;

    let file_name = jar
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .ok_or_else(|| AppError::Other("that file has no name".into()))?;
    if !is_jar(&file_name) {
        return Err(AppError::Other(format!("{file_name} is not a .jar file")));
    }

    let target = folder.join(base_name(&file_name));
    if target.exists() {
        return Err(AppError::Other(format!(
            "{} is already installed in this instance",
            base_name(&file_name)
        )));
    }
    tokio::fs::copy(jar, &target).await.ctx("copy jar", &target)?;

    let path = target.clone();
    let metadata = tokio::task::spawn_blocking(move || jarmeta::read_jar(&path))
        .await
        .map_err(|e| AppError::internal("reading the jar", e))?
        .unwrap_or(None);

    let size = tokio::fs::metadata(&target).await.map(|m| m.len()).unwrap_or(0);
    let now = now_rfc3339();
    sqlx::query(
        "INSERT INTO mods (instance_id, target_dir, file_name, display_name, version, loader,
            mc_version, source, size_bytes, enabled, pinned, installed_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, 'local', ?, 1, 0, ?, ?)
         ON CONFLICT(instance_id, target_dir, file_name) DO UPDATE SET
            display_name = excluded.display_name, version = excluded.version,
            size_bytes = excluded.size_bytes, updated_at = excluded.updated_at",
    )
    .bind(id)
    .bind(
        Loader::for_server_type(row.server_type)
            .map(|loader| loader.content_dir())
            .unwrap_or("mods"),
    )
    .bind(base_name(&file_name))
    .bind(metadata.as_ref().and_then(|meta| meta.name.clone()))
    .bind(metadata.as_ref().and_then(|meta| meta.version.clone()))
    .bind(metadata.as_ref().and_then(|meta| meta.loaders.first().cloned()))
    .bind(metadata.as_ref().and_then(|meta| meta.game_versions.first().cloned()))
    .bind(size as i64)
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await?;

    record_event(
        &state.db,
        id,
        "mods",
        Some(&format!("installed {} from a local file", base_name(&file_name))),
    )
    .await?;

    let loader = Loader::for_server_type(row.server_type).unwrap_or(Loader::Fabric);
    let mismatch = metadata
        .as_ref()
        .map(|meta| jarmeta::check(meta, loader, &row.mc_version))
        .filter(|mismatch| !mismatch.is_empty());

    Ok(ModView {
        file_name: base_name(&file_name),
        enabled: true,
        size_bytes: size,
        tracked: None,
        metadata,
        mismatch,
        required_by: Vec::new(),
    })
}

/// Downloads and records one planned mod. Used by both the install flow and the
/// pack import.
/// Whether a file URL is one this app will download from.
///
/// Each source has its own CDN, and a version resolved from one must not be
/// able to point the downloader at the other's — or anywhere else.
pub fn download_host_allowed(source: SourceId, url: &str) -> bool {
    match source {
        SourceId::CurseForge => {
            let host = crate::mods::ratelimit::host_of(url);
            url.starts_with("https://")
                && ["forgecdn.net", "curseforge.com"]
                    .iter()
                    .any(|allowed| host == *allowed || host.ends_with(&format!(".{allowed}")))
        }
        _ => modrinth::host_allowed(url),
    }
}

pub async fn install_planned(
    state: &AppState,
    id: i64,
    planned: &PlannedMod,
    version: &SourceVersion,
    cancel: &CancellationToken,
) -> AppResult<()> {
    let row = instance::get(&state.db, id).await?;
    let folder = content_dir(&row)?;
    tokio::fs::create_dir_all(&folder)
        .await
        .ctx("create content folder", &folder)?;

    let file = version
        .primary_file()
        .ok_or_else(|| AppError::Other(format!("{} has no downloadable file", planned.project_title)))?;
    if !download_host_allowed(version.source, &file.url) {
        return Err(AppError::Other(format!(
            "{} would download from {}, which is not an allowed host",
            planned.file_name, file.url
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

    let target = folder.join(&file.file_name);
    crate::download::download(&state.http, &artifact, &target, cancel, |_| {}).await?;

    // Another version of the same project may already be installed under a
    // different file name — switching version, or going back to an older one.
    // Leaving both would load two copies of the same mod, which is a crash.
    replace_other_versions(state, id, &version.project_id, &file.file_name, &folder).await?;

    let now = now_rfc3339();
    let mod_id: i64 = sqlx::query_scalar(
        "INSERT INTO mods (instance_id, target_dir, file_name, display_name, version, loader,
            mc_version, source, project_id, version_id, page_url, sha1, sha512, size_bytes,
            enabled, pinned, installed_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, 0, ?, ?)
         ON CONFLICT(instance_id, target_dir, file_name) DO UPDATE SET
            version = excluded.version, version_id = excluded.version_id,
            sha512 = excluded.sha512, size_bytes = excluded.size_bytes,
            update_version_id = NULL, updated_at = excluded.updated_at
         RETURNING id",
    )
    .bind(id)
    .bind(
        Loader::for_server_type(row.server_type)
            .map(|loader| loader.content_dir())
            .unwrap_or("mods"),
    )
    .bind(&file.file_name)
    .bind(&planned.project_title)
    .bind(&version.version_number)
    .bind(version.loaders.first().cloned())
    .bind(version.game_versions.first().cloned())
    .bind(version.source.as_str())
    .bind(&version.project_id)
    .bind(&version.id)
    .bind(planned.page_url.clone())
    .bind(&file.sha1)
    .bind(&file.sha512)
    .bind(file.size.map(|size| size as i64))
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.db)
    .await?;

    // Record what this mod needs, so uninstall can warn about it later.
    sqlx::query("DELETE FROM mod_dependencies WHERE mod_id = ?")
        .bind(mod_id)
        .execute(&state.db)
        .await?;
    for dependency in &version.dependencies {
        let Some(project) = &dependency.project_id else {
            continue;
        };
        let required = dependency.kind == source::DependencyKind::Required;
        sqlx::query(
            "INSERT INTO mod_dependencies (mod_id, dep_project_id, dep_version_id, required)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(mod_id, dep_project_id) DO UPDATE SET required = excluded.required",
        )
        .bind(mod_id)
        .bind(project)
        .bind(&dependency.version_id)
        .bind(required)
        .execute(&state.db)
        .await?;
    }

    Ok(())
}

/// One tracked mod, as the update check sees it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TrackedMod {
    pub id: i64,
    pub file_name: String,
    pub project_id: Option<String>,
    pub version_id: Option<String>,
    pub pinned: bool,
    /// Which source installed it, as stored in `mods.source`.
    pub source: String,
}

/// Mods this app installed from a source, which are the only ones it can check
/// for updates.
/// Removes any other file of the same project, on disk and in the table.
///
/// Switching to a different version is the normal way this happens, and it must
/// leave exactly one jar behind — including when the previous one was disabled,
/// which is the `.jar.disabled` name.
async fn replace_other_versions(
    state: &AppState,
    id: i64,
    project_id: &str,
    keep_file_name: &str,
    folder: &Path,
) -> AppResult<()> {
    let others: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, file_name FROM mods
         WHERE instance_id = ? AND project_id = ? AND file_name != ?",
    )
    .bind(id)
    .bind(project_id)
    .bind(keep_file_name)
    .fetch_all(&state.db)
    .await?;

    for (row_id, file_name) in others {
        for candidate in [folder.join(&file_name), folder.join(format!("{file_name}.disabled"))] {
            if candidate.is_file() {
                tokio::fs::remove_file(&candidate)
                    .await
                    .ctx("remove the previous version", &candidate)?;
            }
        }
        sqlx::query("DELETE FROM mods WHERE id = ?")
            .bind(row_id)
            .execute(&state.db)
            .await?;
        tracing::info!(project = project_id, replaced = %file_name, "switched mod version");
    }
    Ok(())
}

pub async fn tracked(state: &AppState, id: i64) -> AppResult<Vec<TrackedMod>> {
    let rows = sqlx::query_as::<_, TrackedMod>(
        "SELECT id, file_name, project_id, version_id, pinned, source
         FROM mods WHERE instance_id = ?",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(rows)
}

/// Records the newest suitable version for every unpinned tracked mod.
///
/// A pinned mod is left exactly where the user put it: pinning exists precisely
/// so an update check cannot move it.
pub async fn check_updates<S: ModSource>(
    state: &AppState,
    id: i64,
    source_id: SourceId,
    source: &S,
    loader: Loader,
    mc_version: &str,
    index: &crate::mcversion::VersionIndex,
) -> AppResult<usize> {
    let mut found = 0usize;

    for entry in tracked(state, id).await? {
        // Only what this source installed: a version id from the other one
        // would either miss or, worse, match something unrelated.
        if entry.source != source_id.as_str() {
            continue;
        }
        let (Some(project_id), Some(version_id)) = (entry.project_id, entry.version_id) else {
            continue;
        };
        if entry.pinned {
            continue;
        }

        let versions = source
            .versions(
                &project_id,
                &VersionFilter {
                    loaders: loader
                        .accepted()
                        .iter()
                        .map(|loader| loader.to_string())
                        .collect(),
                    game_versions: vec![mc_version.to_string()],
                },
            )
            .await?;

        let update = resolve::pick_version(&versions, loader, mc_version, index)
            .filter(|version| version.id != version_id)
            .map(|version| version.id);
        if update.is_some() {
            found += 1;
        }

        sqlx::query("UPDATE mods SET update_version_id = ? WHERE id = ?")
            .bind(&update)
            .bind(entry.id)
            .execute(&state.db)
            .await?;
    }

    Ok(found)
}

/// The loader an instance uses, or an error naming what to do about vanilla.
pub fn loader_of(server_type: ServerType, name: &str) -> AppResult<Loader> {
    Loader::for_server_type(server_type).ok_or_else(|| {
        AppError::Other(format!(
            "\"{name}\" is a vanilla server, which loads no mods or plugins."
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jar_names_are_recognised_including_disabled_ones() {
        assert!(is_jar("sodium.jar"));
        assert!(is_jar("Sodium.JAR"));
        assert!(is_jar("sodium.jar.disabled"));
        assert!(!is_jar("readme.txt"));
        assert!(!is_jar("sodium.jar.bak"));
    }

    #[test]
    fn the_base_name_ignores_the_disabled_suffix() {
        assert_eq!(base_name("sodium.jar"), "sodium.jar");
        assert_eq!(base_name("sodium.jar.disabled"), "sodium.jar");
        assert!(is_enabled("sodium.jar"));
        assert!(!is_enabled("sodium.jar.disabled"));
    }

    #[test]
    fn renaming_toggles_the_suffix_without_stacking_it() {
        assert_eq!(name_for("sodium.jar", false), "sodium.jar.disabled");
        assert_eq!(name_for("sodium.jar.disabled", false), "sodium.jar.disabled");
        assert_eq!(name_for("sodium.jar.disabled", true), "sodium.jar");
        assert_eq!(name_for("sodium.jar", true), "sodium.jar");
    }

    #[tokio::test]
    async fn the_content_folder_follows_the_server_type() {
        let dir = tempfile::tempdir().unwrap();
        let make = |server_type: ServerType| {
            let mut instance = sample(dir.path());
            instance.server_type = server_type;
            instance
        };

        assert!(content_dir(&make(ServerType::Paper))
            .unwrap()
            .ends_with("plugins"));
        assert!(content_dir(&make(ServerType::Purpur))
            .unwrap()
            .ends_with("plugins"));
        for server_type in [ServerType::Fabric, ServerType::Forge, ServerType::NeoForge] {
            assert!(content_dir(&make(server_type)).unwrap().ends_with("mods"));
        }
    }

    #[tokio::test]
    async fn vanilla_is_refused_with_an_explanation_not_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        let mut instance = sample(dir.path());
        instance.server_type = ServerType::Vanilla;

        let err = content_dir(&instance).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("vanilla"), "{message}");
        assert!(message.contains("Fabric") && message.contains("Paper"), "{message}");
        assert!(!dir.path().join("mods").exists(), "nothing was created");
    }

    fn sample(path: &Path) -> Instance {
        Instance {
            id: 1,
            uuid: "u".into(),
            name: "Test".into(),
            path: path.to_string_lossy().to_string(),
            server_type: ServerType::Fabric,
            mc_version: "1.21.4".into(),
            loader_version: None,
            launch_kind: crate::db::models::LaunchKind::Jar,
            launch_target: Some("server.jar".into()),
            java_path: None,
            java_major: Some(21),
            jvm_args: "[]".into(),
            server_args: "[]".into(),
            min_ram_mb: 1024,
            max_ram_mb: 4096,
            eula_accepted: true,
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
            map_kind: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[tokio::test]
    async fn installing_another_version_replaces_the_one_that_is_there() {
        // Switching version — or going back to an older one — must leave one
        // jar. Two files of the same mod in `mods/` is a crash on boot.
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("mods");
        std::fs::create_dir_all(&folder).unwrap();

        let pool = crate::db::connect_in_memory().await.unwrap();
        let state = AppState::new(pool, dir.path().to_path_buf());
        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                jvm_args, server_args, created_at, updated_at)
             VALUES ('u1', 'A', ?, 'fabric', '1.21.4', 'jar', '[]', '[]', ?, ?)",
        )
        .bind(dir.path().to_string_lossy().to_string())
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap();

        // An older version is installed, and disabled at that.
        let old = folder.join("sodium-0.5.jar");
        std::fs::write(old.with_extension("jar.disabled"), b"old").unwrap();
        sqlx::query(
            "INSERT INTO mods (instance_id, target_dir, file_name, display_name, source,
                project_id, version_id, enabled, pinned, installed_at, updated_at)
             VALUES (1, 'mods', 'sodium-0.5.jar', 'Sodium', 'modrinth', 'AANobbMI', 'v-old',
                0, 0, ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap();

        // The new one lands, and the old row and file go.
        std::fs::write(folder.join("sodium-0.6.jar"), b"new").unwrap();
        replace_other_versions(&state, 1, "AANobbMI", "sodium-0.6.jar", &folder)
            .await
            .unwrap();

        assert!(!old.with_extension("jar.disabled").exists(), "the disabled jar went too");
        assert!(folder.join("sodium-0.6.jar").is_file());
        let rows: Vec<String> = sqlx::query_scalar("SELECT file_name FROM mods WHERE instance_id = 1")
            .fetch_all(&state.db)
            .await
            .unwrap();
        assert!(rows.is_empty(), "the old row is gone: {rows:?}");
    }

    #[tokio::test]
    async fn another_project_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("mods");
        std::fs::create_dir_all(&folder).unwrap();

        let pool = crate::db::connect_in_memory().await.unwrap();
        let state = AppState::new(pool, dir.path().to_path_buf());
        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                jvm_args, server_args, created_at, updated_at)
             VALUES ('u1', 'A', 'Z:/a', 'fabric', '1.21.4', 'jar', '[]', '[]', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap();

        std::fs::write(folder.join("lithium.jar"), b"other").unwrap();
        sqlx::query(
            "INSERT INTO mods (instance_id, target_dir, file_name, display_name, source,
                project_id, version_id, enabled, pinned, installed_at, updated_at)
             VALUES (1, 'mods', 'lithium.jar', 'Lithium', 'modrinth', 'gvQqBUqZ', 'v1', 1, 0, ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap();

        replace_other_versions(&state, 1, "AANobbMI", "sodium-0.6.jar", &folder)
            .await
            .unwrap();

        assert!(folder.join("lithium.jar").is_file(), "a different mod is untouched");
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM mods")
                .fetch_one(&state.db)
                .await
                .unwrap(),
            1
        );
    }

    #[test]
    fn each_source_downloads_only_from_its_own_hosts() {
        // A version resolved from one source must not be able to point the
        // downloader at the other's CDN, or anywhere else.
        assert!(download_host_allowed(
            SourceId::Modrinth,
            "https://cdn.modrinth.com/data/AANobbMI/versions/x/sodium.jar"
        ));
        assert!(download_host_allowed(
            SourceId::CurseForge,
            "https://edge.forgecdn.net/files/5300/0/jei.jar"
        ));

        assert!(!download_host_allowed(
            SourceId::CurseForge,
            "https://cdn.modrinth.com/data/x/sodium.jar"
        ));
        assert!(!download_host_allowed(
            SourceId::Modrinth,
            "https://edge.forgecdn.net/files/5300/0/jei.jar"
        ));
        assert!(!download_host_allowed(SourceId::CurseForge, "http://edge.forgecdn.net/x.jar"));
        assert!(!download_host_allowed(SourceId::CurseForge, "https://evil.example.com/x.jar"));
    }
}
