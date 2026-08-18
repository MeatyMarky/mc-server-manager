//! Adopting an existing server folder as an instance.
//!
//! Detection is deliberately split into pure functions over file names and file
//! contents, with one thin filesystem pass on top, so the guessing rules are
//! testable without fixtures on disk.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::db::models::{
    default_jvm_args, default_server_args, Instance, InstanceManifest, LaunchKind, ServerType,
};
use crate::db::{now_rfc3339, record_event};
use crate::error::{AppError, AppResult};
use crate::paths;
use crate::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub enum DetectConfidence {
    /// Read straight out of `.msm/instance.json`, or an unambiguous jar name.
    High,
    /// Inferred from one strong signal (a launcher jar, a libraries path).
    Medium,
    /// It is a server folder, but the type or version is a guess.
    Low,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct ImportCandidate {
    pub path: String,
    pub suggested_name: String,
    pub server_type: ServerType,
    pub mc_version: Option<String>,
    pub loader_version: Option<String>,
    pub launch_kind: LaunchKind,
    pub launch_target: Option<String>,
    pub eula_accepted: bool,
    pub worlds: Vec<String>,
    pub confidence: DetectConfidence,
    /// Whether a previous install of this app already managed the folder.
    pub from_manifest: bool,
    /// Human-readable reasons, shown in the confirmation dialog.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct ImportInstanceInput {
    pub path: String,
    pub name: String,
    pub server_type: ServerType,
    pub mc_version: String,
    pub loader_version: Option<String>,
}

/// What a jar file name reveals. `None` means the name says nothing useful.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JarHint {
    pub server_type: ServerType,
    pub mc_version: Option<String>,
    pub loader_version: Option<String>,
}

/// Recognizes the file names the six supported server types actually ship with.
pub fn classify_jar_name(file_name: &str) -> Option<JarHint> {
    let lower = file_name.to_ascii_lowercase();
    if !lower.ends_with(".jar") {
        return None;
    }
    let stem = &lower[..lower.len() - 4];

    // fabric-server-mc.1.20.1-loader.0.15.7-launcher.1.0.1.jar
    if stem.starts_with("fabric-server") {
        return Some(JarHint {
            server_type: ServerType::Fabric,
            mc_version: segment_after(stem, "mc."),
            loader_version: segment_after(stem, "loader."),
        });
    }

    // paper-1.20.4-496.jar / purpur-1.21.4-2312.jar
    for (prefix, server_type) in [
        ("paper-", ServerType::Paper),
        ("purpur-", ServerType::Purpur),
    ] {
        if let Some(rest) = stem.strip_prefix(prefix) {
            let mut parts = rest.split('-');
            let mc = parts.next().map(str::to_string).filter(|s| looks_like_mc_version(s));
            let build = parts.next().map(str::to_string);
            return Some(JarHint {
                server_type,
                mc_version: mc,
                loader_version: build,
            });
        }
    }

    // forge-1.20.1-47.2.0-installer.jar (or -universal/-shim)
    if let Some(rest) = stem.strip_prefix("forge-") {
        let mut parts = rest.split('-');
        let mc = parts.next().map(str::to_string).filter(|s| looks_like_mc_version(s));
        let forge = parts.next().map(str::to_string);
        return Some(JarHint {
            server_type: ServerType::Forge,
            mc_version: mc,
            loader_version: forge,
        });
    }

    // neoforge-21.1.65-installer.jar — the name carries no Minecraft version.
    if let Some(rest) = stem.strip_prefix("neoforge-") {
        let version = rest.split('-').next().map(str::to_string);
        return Some(JarHint {
            server_type: ServerType::NeoForge,
            mc_version: version.as_deref().and_then(neoforge_to_mc_version),
            loader_version: version,
        });
    }

    // minecraft_server.1.20.1.jar / server-1.20.1.jar
    if let Some(rest) = stem.strip_prefix("minecraft_server.") {
        return Some(JarHint {
            server_type: ServerType::Vanilla,
            mc_version: Some(rest.to_string()).filter(|s| looks_like_mc_version(s)),
            loader_version: None,
        });
    }
    if stem == "server" {
        return Some(JarHint {
            server_type: ServerType::Vanilla,
            mc_version: None,
            loader_version: None,
        });
    }

    None
}

/// Delegates to the NeoForge provider, which owns the version scheme for both
/// the classic (`21.1.65` -> 1.21.1) and calendar (`26.2.0.62` -> 26.2) eras.
pub fn neoforge_to_mc_version(neoforge_version: &str) -> Option<String> {
    crate::providers::neoforge::mc_version_for(neoforge_version)
}

/// Paper and Purpur write `version_history.json`; the line looks like
/// `"currentVersion": "git-Paper-196 (MC: 1.20.4)"` or `"1.21.4-118-abc (MC: 1.21.4)"`.
pub fn parse_version_history(contents: &str) -> Option<(String, Option<String>)> {
    let value: serde_json::Value = serde_json::from_str(contents).ok()?;
    let current = value.get("currentVersion")?.as_str()?;

    let mc = current
        .split("(MC:")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .map(|s| s.trim().to_string())
        .filter(|s| looks_like_mc_version(s))?;

    let build = current
        .split_whitespace()
        .next()
        .and_then(|head| head.split('-').find(|p| p.chars().all(|c| c.is_ascii_digit())))
        .map(str::to_string);

    Some((mc, build))
}

/// Forge and NeoForge unpack into `libraries/net/<vendor>/<artifact>/<version>/`.
/// That version folder is the most reliable source for a modded server folder.
pub fn parse_loader_from_libraries(relative: &Path) -> Option<(ServerType, Option<String>, String)> {
    let parts: Vec<String> = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect();
    let idx = parts.iter().position(|p| p == "net")?;
    let vendor = parts.get(idx + 1)?.as_str();
    let artifact = parts.get(idx + 2)?.as_str();
    let version = parts.get(idx + 3)?.clone();

    match (vendor, artifact) {
        ("minecraftforge", "forge") => {
            // 1.20.1-47.2.0
            let mut split = version.splitn(2, '-');
            let mc = split.next().map(str::to_string).filter(|s| looks_like_mc_version(s));
            let forge = split.next().unwrap_or_default().to_string();
            Some((ServerType::Forge, mc, forge))
        }
        ("neoforged", "neoforge") => {
            let mc = neoforge_to_mc_version(&version);
            Some((ServerType::NeoForge, mc, version))
        }
        _ => None,
    }
}

/// `eula=true` (ignoring comments and whitespace).
pub fn parse_eula(contents: &str) -> bool {
    contents
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .any(|(k, v)| k.trim() == "eula" && v.trim().eq_ignore_ascii_case("true"))
}

fn segment_after(stem: &str, marker: &str) -> Option<String> {
    let start = stem.find(marker)? + marker.len();
    let rest = &stem[start..];
    let end = rest.find('-').unwrap_or(rest.len());
    Some(rest[..end].to_string()).filter(|s| !s.is_empty())
}

fn looks_like_mc_version(s: &str) -> bool {
    // Both eras: 1.21.4 and 26.2 are equally valid.
    crate::mcversion::looks_like_version(s)
}

/// Inspects a folder and proposes what to import. Never mutates anything.
pub fn detect(path: &Path) -> AppResult<ImportCandidate> {
    if !path.is_dir() {
        return Err(AppError::io(
            "open folder",
            path,
            std::io::Error::new(std::io::ErrorKind::NotFound, "no such folder"),
        ));
    }

    let mut notes = Vec::new();
    let suggested_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "Imported server".to_string());

    // The manifest is only consulted here: on import, or when a folder has no row.
    let manifest = read_manifest(path);
    if let Some(m) = &manifest {
        notes.push("Found .msm/instance.json from a previous install".to_string());
        return Ok(ImportCandidate {
            path: path.to_string_lossy().to_string(),
            suggested_name: m.name.clone(),
            server_type: m.server_type,
            mc_version: Some(m.mc_version.clone()),
            loader_version: m.loader_version.clone(),
            launch_kind: m.launch_kind,
            launch_target: m.launch_target.clone(),
            eula_accepted: eula_accepted(path),
            worlds: super::crud::world_dir_names(path),
            confidence: DetectConfidence::High,
            from_manifest: true,
            notes,
        });
    }

    let entries: Vec<String> = std::fs::read_dir(path)
        .map_err(|e| AppError::io("read folder", path, e))?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();

    let mut server_type = None;
    let mut mc_version = None;
    let mut loader_version = None;
    let mut launch_kind = LaunchKind::Jar;
    let mut launch_target = None;
    let mut confidence = DetectConfidence::Low;

    // 1. A libraries tree is the strongest signal for Forge/NeoForge.
    if let Some((kind, mc, loader)) = scan_libraries(path) {
        server_type = Some(kind);
        mc_version = mc;
        loader_version = Some(loader);
        launch_kind = LaunchKind::Script;
        launch_target = script_name(path, &entries);
        confidence = DetectConfidence::Medium;
        notes.push("Detected an installed mod loader under libraries/".to_string());
    }

    // 2. Jar names.
    if server_type.is_none() {
        for name in &entries {
            if let Some(hint) = classify_jar_name(name) {
                server_type = Some(hint.server_type);
                mc_version = hint.mc_version;
                loader_version = hint.loader_version;
                launch_target = Some(name.clone());
                confidence = if name.eq_ignore_ascii_case("server.jar") {
                    DetectConfidence::Low
                } else {
                    DetectConfidence::High
                };
                notes.push(format!("Matched the server jar {name}"));
                break;
            }
        }
    }

    // 3. version_history.json pins the Minecraft version for Paper/Purpur.
    if let Ok(contents) = std::fs::read_to_string(path.join("version_history.json")) {
        if let Some((mc, build)) = parse_version_history(&contents) {
            if mc_version.is_none() {
                mc_version = Some(mc.clone());
            }
            if loader_version.is_none() {
                loader_version = build;
            }
            if server_type.is_none() {
                server_type = Some(ServerType::Paper);
            }
            confidence = DetectConfidence::High;
            notes.push(format!("version_history.json reports Minecraft {mc}"));
        }
    }

    // 4. Last resort: the content folder tells us the family.
    if server_type.is_none() {
        if path.join("plugins").is_dir() {
            server_type = Some(ServerType::Paper);
            notes.push("Found plugins/, assuming a Paper-family server".to_string());
        } else if path.join("mods").is_dir() {
            server_type = Some(ServerType::Fabric);
            notes.push("Found mods/, assuming Fabric".to_string());
        }
    }

    let is_server_folder = server_type.is_some()
        || path.join("server.properties").is_file()
        || paths::eula_path(path).is_file();
    if !is_server_folder {
        return Err(AppError::NotAServerFolder(path.to_path_buf()));
    }

    if mc_version.is_none() {
        notes.push("Could not determine the Minecraft version — please confirm it".to_string());
    }

    Ok(ImportCandidate {
        path: path.to_string_lossy().to_string(),
        suggested_name,
        server_type: server_type.unwrap_or(ServerType::Vanilla),
        mc_version,
        loader_version,
        launch_kind,
        launch_target,
        eula_accepted: eula_accepted(path),
        worlds: super::crud::world_dir_names(path),
        confidence,
        from_manifest: false,
        notes,
    })
}

fn script_name(path: &Path, entries: &[String]) -> Option<String> {
    let wanted = if cfg!(windows) { "run.bat" } else { "run.sh" };
    entries
        .iter()
        .find(|e| e.eq_ignore_ascii_case(wanted))
        .cloned()
        .filter(|name| path.join(name).is_file())
}

fn scan_libraries(path: &Path) -> Option<(ServerType, Option<String>, String)> {
    let libraries = path.join("libraries");
    if !libraries.is_dir() {
        return None;
    }
    for entry in walkdir::WalkDir::new(&libraries)
        .min_depth(4)
        .max_depth(4)
        .into_iter()
        .flatten()
    {
        if !entry.file_type().is_dir() {
            continue;
        }
        let relative = entry.path().strip_prefix(&libraries).ok()?;
        if let Some(found) = parse_loader_from_libraries(relative) {
            return Some(found);
        }
    }
    None
}

fn eula_accepted(path: &Path) -> bool {
    std::fs::read_to_string(paths::eula_path(path))
        .map(|c| parse_eula(&c))
        .unwrap_or(false)
}

fn read_manifest(path: &Path) -> Option<InstanceManifest> {
    let raw = std::fs::read_to_string(paths::instance_json_path(path)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Adopts the folder. The user has already confirmed (and possibly corrected)
/// the detected values in the import dialog.
pub async fn import(state: &AppState, input: ImportInstanceInput) -> AppResult<Instance> {
    let path = PathBuf::from(&input.path);
    paths::validate_instance_name(&input.name)?;
    if !path.is_dir() {
        return Err(AppError::io(
            "open folder",
            &path,
            std::io::Error::new(std::io::ErrorKind::NotFound, "no such folder"),
        ));
    }

    let existing: Vec<(i64, String, String)> = sqlx::query_as("SELECT id, name, path FROM instances")
        .fetch_all(&state.db)
        .await?;
    for (_, name, existing_path) in &existing {
        if paths::same_path(Path::new(existing_path), &path) {
            return Err(AppError::PathInUse(PathBuf::from(existing_path)));
        }
        if name.eq_ignore_ascii_case(input.name.trim()) {
            return Err(AppError::NameInUse(input.name.trim().to_string()));
        }
    }

    let candidate = detect(&path)?;
    // Our metadata folder is created on import; the server's own files are untouched.
    super::crud::scaffold(&path, input.server_type).await?;

    let now = now_rfc3339();
    let uuid = uuid::Uuid::new_v4().to_string();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO instances (
            uuid, name, path, server_type, mc_version, loader_version,
            launch_kind, launch_target, jvm_args, server_args,
            eula_accepted, eula_accepted_at, created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         RETURNING id",
    )
    .bind(&uuid)
    .bind(input.name.trim())
    .bind(paths::normalize(&path).to_string_lossy().to_string())
    .bind(input.server_type)
    .bind(&input.mc_version)
    .bind(input.loader_version.or(candidate.loader_version))
    .bind(candidate.launch_kind)
    .bind(&candidate.launch_target)
    .bind(serde_json::to_string(&default_jvm_args())?)
    .bind(serde_json::to_string(&default_server_args())?)
    .bind(candidate.eula_accepted)
    .bind(candidate.eula_accepted.then(|| now.clone()))
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.db)
    .await?;

    let instance = super::get(&state.db, id).await?;
    super::crud::write_manifest(&instance).await?;
    record_event(&state.db, id, "imported", Some(&instance.path)).await?;
    Ok(instance)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_paper_and_purpur_jars() {
        let hint = classify_jar_name("paper-1.20.4-496.jar").unwrap();
        assert_eq!(hint.server_type, ServerType::Paper);
        assert_eq!(hint.mc_version.as_deref(), Some("1.20.4"));
        assert_eq!(hint.loader_version.as_deref(), Some("496"));

        let hint = classify_jar_name("purpur-1.21.4-2312.jar").unwrap();
        assert_eq!(hint.server_type, ServerType::Purpur);
        assert_eq!(hint.mc_version.as_deref(), Some("1.21.4"));
    }

    #[test]
    fn recognizes_fabric_launcher_jars() {
        let hint =
            classify_jar_name("fabric-server-mc.1.20.1-loader.0.15.7-launcher.1.0.1.jar").unwrap();
        assert_eq!(hint.server_type, ServerType::Fabric);
        assert_eq!(hint.mc_version.as_deref(), Some("1.20.1"));
        assert_eq!(hint.loader_version.as_deref(), Some("0.15.7"));
    }

    #[test]
    fn recognizes_forge_and_neoforge_installers() {
        let hint = classify_jar_name("forge-1.20.1-47.2.0-installer.jar").unwrap();
        assert_eq!(hint.server_type, ServerType::Forge);
        assert_eq!(hint.mc_version.as_deref(), Some("1.20.1"));
        assert_eq!(hint.loader_version.as_deref(), Some("47.2.0"));

        let hint = classify_jar_name("neoforge-21.1.65-installer.jar").unwrap();
        assert_eq!(hint.server_type, ServerType::NeoForge);
        assert_eq!(hint.mc_version.as_deref(), Some("1.21.1"));
        assert_eq!(hint.loader_version.as_deref(), Some("21.1.65"));
    }

    #[test]
    fn recognizes_vanilla_jars_and_ignores_the_rest() {
        let hint = classify_jar_name("minecraft_server.1.20.1.jar").unwrap();
        assert_eq!(hint.server_type, ServerType::Vanilla);
        assert_eq!(hint.mc_version.as_deref(), Some("1.20.1"));

        let hint = classify_jar_name("server.jar").unwrap();
        assert_eq!(hint.server_type, ServerType::Vanilla);
        assert!(hint.mc_version.is_none());

        assert!(classify_jar_name("sodium-fabric-0.5.jar").is_none());
        assert!(classify_jar_name("notes.txt").is_none());
    }

    #[test]
    fn neoforge_versions_map_to_minecraft_versions() {
        assert_eq!(neoforge_to_mc_version("21.1.65").as_deref(), Some("1.21.1"));
        assert_eq!(neoforge_to_mc_version("20.4.237").as_deref(), Some("1.20.4"));
        // A zero patch means the .0 release of that minor: 21.0.x is Minecraft 1.21.
        assert_eq!(neoforge_to_mc_version("21.0.167").as_deref(), Some("1.21"));
        assert_eq!(neoforge_to_mc_version("nonsense"), None);
    }

    #[test]
    fn parses_paper_version_history() {
        let json = r#"{"currentVersion": "git-Paper-196 (MC: 1.20.4)"}"#;
        assert_eq!(
            parse_version_history(json),
            Some(("1.20.4".to_string(), Some("196".to_string())))
        );

        let json = r#"{"currentVersion": "1.21.4-118-abcdef (MC: 1.21.4)"}"#;
        let (mc, _) = parse_version_history(json).unwrap();
        assert_eq!(mc, "1.21.4");

        assert!(parse_version_history("{}").is_none());
        assert!(parse_version_history("not json").is_none());
    }

    #[test]
    fn parses_loader_versions_from_library_paths() {
        // Built component-wise: the separator differs per platform.
        let forge: PathBuf = ["net", "minecraftforge", "forge", "1.20.1-47.2.0"]
            .iter()
            .collect();
        let (kind, mc, loader) = parse_loader_from_libraries(&forge).unwrap();
        assert_eq!(kind, ServerType::Forge);
        assert_eq!(mc.as_deref(), Some("1.20.1"));
        assert_eq!(loader, "47.2.0");

        let neo: PathBuf = ["net", "neoforged", "neoforge", "21.1.65"].iter().collect();
        let (kind, mc, loader) = parse_loader_from_libraries(&neo).unwrap();
        assert_eq!(kind, ServerType::NeoForge);
        assert_eq!(mc.as_deref(), Some("1.21.1"));
        assert_eq!(loader, "21.1.65");

        let other: PathBuf = ["net", "fabricmc", "intermediary", "1.20.1"].iter().collect();
        assert!(parse_loader_from_libraries(&other).is_none());
    }

    #[test]
    fn parses_eula_flag() {
        assert!(parse_eula("#comment\neula=true\n"));
        assert!(parse_eula("eula=TRUE"));
        assert!(!parse_eula("eula=false"));
        assert!(!parse_eula("#eula=true"));
        assert!(!parse_eula(""));
    }

    #[test]
    fn detects_a_paper_folder() {
        let dir = temp_dir();
        std::fs::write(dir.join("paper-1.20.4-496.jar"), b"jar").unwrap();
        std::fs::write(dir.join("server.properties"), b"motd=hi").unwrap();
        std::fs::write(paths::eula_path(&dir), b"eula=true").unwrap();
        std::fs::create_dir_all(dir.join("world")).unwrap();
        std::fs::write(dir.join("world").join("level.dat"), b"x").unwrap();

        let found = detect(&dir).unwrap();
        assert_eq!(found.server_type, ServerType::Paper);
        assert_eq!(found.mc_version.as_deref(), Some("1.20.4"));
        assert!(found.eula_accepted);
        assert_eq!(found.worlds, vec!["world".to_string()]);
        assert_eq!(found.confidence, DetectConfidence::High);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn detects_a_neoforge_folder_from_libraries() {
        let dir = temp_dir();
        let lib: PathBuf = ["libraries", "net", "neoforged", "neoforge", "21.1.65"]
            .iter()
            .collect();
        std::fs::create_dir_all(dir.join(lib)).unwrap();
        std::fs::write(dir.join("server.properties"), b"").unwrap();

        let found = detect(&dir).unwrap();
        assert_eq!(found.server_type, ServerType::NeoForge);
        assert_eq!(found.mc_version.as_deref(), Some("1.21.1"));
        assert_eq!(found.launch_kind, LaunchKind::Script);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn rejects_folders_that_are_not_servers() {
        let dir = temp_dir();
        std::fs::write(dir.join("holiday.jpg"), b"x").unwrap();
        let err = detect(&dir).unwrap_err();
        assert_eq!(err.kind(), "not_a_server_folder");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_manifest_short_circuits_detection() {
        let dir = temp_dir();
        std::fs::create_dir_all(paths::msm_dir(&dir)).unwrap();
        let manifest = InstanceManifest {
            schema: 1,
            uuid: "abc".into(),
            name: "Old Name".into(),
            server_type: ServerType::Fabric,
            mc_version: "1.20.1".into(),
            loader_version: Some("0.15.7".into()),
            launch_kind: LaunchKind::Jar,
            launch_target: Some("server.jar".into()),
            jvm_args: vec![],
            server_args: vec![],
            min_ram_mb: 1024,
            max_ram_mb: 4096,
            java_path: None,
            eula_accepted: true,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        };
        std::fs::write(
            paths::instance_json_path(&dir),
            serde_json::to_string(&manifest).unwrap(),
        )
        .unwrap();

        let found = detect(&dir).unwrap();
        assert!(found.from_manifest);
        assert_eq!(found.suggested_name, "Old Name");
        assert_eq!(found.server_type, ServerType::Fabric);
        assert_eq!(found.confidence, DetectConfidence::High);
        std::fs::remove_dir_all(dir).ok();
    }

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("msm-import-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
