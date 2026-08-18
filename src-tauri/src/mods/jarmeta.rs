//! Reading a jar's own metadata.
//!
//! Four formats, because four ecosystems:
//!   * `fabric.mod.json`               — Fabric (and Quilt, which also reads it)
//!   * `META-INF/mods.toml`            — Forge
//!   * `META-INF/neoforge.mods.toml`   — NeoForge, which moved the file
//!   * `plugin.yml` / `paper-plugin.yml` — Bukkit family
//!
//! A jar that declares a different loader or Minecraft version than the instance
//! is reported as a mismatch and still installable: the declaration is often
//! conservative, and refusing would be wrong more often than warning.

use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{AppError, AppResult};
use crate::mcversion;

use super::source::Loader;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct JarMetadata {
    /// Which file the metadata came from, for the UI to explain itself.
    pub format: String,
    pub id: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub authors: Vec<String>,
    /// Loader identifiers the jar declares.
    pub loaders: Vec<String>,
    /// Minecraft versions or ranges the jar declares, as written.
    pub game_versions: Vec<String>,
}

impl JarMetadata {
    fn empty(format: &str) -> Self {
        Self {
            format: format.to_string(),
            id: None,
            name: None,
            version: None,
            description: None,
            authors: Vec::new(),
            loaders: Vec::new(),
            game_versions: Vec::new(),
        }
    }
}

/// Why a jar might not suit the instance. Warnings, never refusals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct Mismatch {
    pub loader: Option<String>,
    pub game_version: Option<String>,
}

impl Mismatch {
    pub fn is_empty(&self) -> bool {
        self.loader.is_none() && self.game_version.is_none()
    }
}

/// Compares what a jar declares with what the instance runs.
pub fn check(metadata: &JarMetadata, loader: Loader, mc_version: &str) -> Mismatch {
    let accepted = loader.accepted();
    // A jar that declares nothing is not claiming to be wrong, so silence is
    // treated the same as a match.
    let loader_ok = metadata.loaders.is_empty()
        || metadata
            .loaders
            .iter()
            .any(|declared| accepted.iter().any(|ok| declared.eq_ignore_ascii_case(ok)));
    let loader_mismatch = (!loader_ok).then(|| {
        format!(
            "this jar declares {} but the instance runs {}",
            metadata.loaders.join(", "),
            loader.as_str()
        )
    });

    // Declared versions are often ranges ("[1.21,1.22)"); an exact match or a
    // range that covers the instance clears the warning.
    let version_ok = metadata.game_versions.is_empty()
        || metadata
            .game_versions
            .iter()
            .any(|declared| version_matches(declared, mc_version));
    let version_mismatch = (!version_ok).then(|| {
        format!(
            "this jar declares Minecraft {} but the instance runs {mc_version}",
            metadata.game_versions.join(", ")
        )
    });

    Mismatch {
        loader: loader_mismatch,
        game_version: version_mismatch,
    }
}

/// True when a declared version or simple range covers `mc_version`.
pub fn version_matches(declared: &str, mc_version: &str) -> bool {
    let declared = declared.trim();
    if declared.is_empty() || declared == "*" {
        return true;
    }
    if declared == mc_version {
        return true;
    }

    // Maven-style ranges: [1.21,1.22) or [1.21.4,).
    if declared.starts_with('[') || declared.starts_with('(') {
        let inner = declared.trim_matches(|c| c == '[' || c == ']' || c == '(' || c == ')');
        let mut bounds = inner.split(',');
        let low = bounds.next().unwrap_or("").trim();
        let high = bounds.next().unwrap_or("").trim();

        let above_low = low.is_empty() || mcversion::at_least(mc_version, low);
        let below_high = high.is_empty() || !mcversion::at_least(mc_version, high);
        return above_low && below_high;
    }

    // ">=1.21" and friends, as Fabric writes them.
    for prefix in [">=", "~", "^", ">"] {
        if let Some(rest) = declared.strip_prefix(prefix) {
            return mcversion::at_least(mc_version, rest.trim());
        }
    }

    false
}

// --- Format parsers --------------------------------------------------------

#[derive(Debug, Deserialize)]
struct FabricModJson {
    id: Option<String>,
    name: Option<String>,
    version: Option<String>,
    description: Option<String>,
    #[serde(default)]
    authors: Vec<serde_json::Value>,
    #[serde(default)]
    depends: std::collections::BTreeMap<String, serde_json::Value>,
}

pub fn parse_fabric(body: &str) -> AppResult<JarMetadata> {
    let parsed: FabricModJson = serde_json::from_str(body)
        .map_err(|e| AppError::Other(format!("fabric.mod.json could not be read: {e}")))?;

    let game_versions = parsed
        .depends
        .get("minecraft")
        .map(|value| match value {
            serde_json::Value::String(text) => vec![text.clone()],
            serde_json::Value::Array(items) => items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect(),
            _ => Vec::new(),
        })
        .unwrap_or_default();

    Ok(JarMetadata {
        format: "fabric.mod.json".to_string(),
        id: parsed.id,
        name: parsed.name,
        version: parsed.version,
        description: parsed.description,
        authors: parsed
            .authors
            .iter()
            .filter_map(|author| match author {
                serde_json::Value::String(name) => Some(name.clone()),
                serde_json::Value::Object(map) => map
                    .get("name")
                    .and_then(|name| name.as_str())
                    .map(str::to_string),
                _ => None,
            })
            .collect(),
        loaders: vec!["fabric".to_string()],
        game_versions,
    })
}

/// Forge and NeoForge share the format; only the file name and the declared
/// loader differ.
pub fn parse_mods_toml(body: &str, neoforge: bool) -> AppResult<JarMetadata> {
    // A document, not a single value: toml's `FromStr for Value` parses the
    // latter, which is why this goes through `from_str::<Table>`.
    let value: toml::Table = toml::from_str(body)
        .map_err(|e| AppError::Other(format!("mods.toml could not be read: {e}")))?;

    let first_mod = value
        .get("mods")
        .and_then(|mods| mods.as_array())
        .and_then(|mods| mods.first());

    let string = |key: &str| -> Option<String> {
        first_mod
            .and_then(|entry| entry.get(key))
            .and_then(|value| value.as_str())
            .map(str::to_string)
    };

    let mod_id = string("modId");
    // Dependencies live under [[dependencies.<modId>]]; the Minecraft entry
    // carries the version range this jar declares.
    let game_versions = mod_id
        .as_deref()
        .and_then(|id| value.get("dependencies")?.get(id)?.as_array().cloned())
        .map(|dependencies| {
            dependencies
                .iter()
                .filter(|dependency| {
                    dependency
                        .get("modId")
                        .and_then(|id| id.as_str())
                        .map(|id| id.eq_ignore_ascii_case("minecraft"))
                        .unwrap_or(false)
                })
                .filter_map(|dependency| {
                    dependency
                        .get("versionRange")
                        .and_then(|range| range.as_str())
                        .map(str::to_string)
                })
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();

    Ok(JarMetadata {
        format: if neoforge {
            "META-INF/neoforge.mods.toml".to_string()
        } else {
            "META-INF/mods.toml".to_string()
        },
        id: mod_id,
        name: string("displayName"),
        version: string("version"),
        description: string("description").map(|text| text.trim().to_string()),
        authors: string("authors").into_iter().collect(),
        loaders: vec![if neoforge { "neoforge" } else { "forge" }.to_string()],
        game_versions,
    })
}

/// `plugin.yml` and `paper-plugin.yml` are flat enough that a full YAML parser
/// is not worth the dependency: only top-level scalars are read.
pub fn parse_plugin_yml(body: &str, paper: bool) -> AppResult<JarMetadata> {
    let mut metadata = JarMetadata::empty(if paper {
        "paper-plugin.yml"
    } else {
        "plugin.yml"
    });
    metadata.loaders = vec!["paper".to_string()];

    for line in body.lines() {
        // Only top-level keys: anything indented belongs to a nested block.
        if line.starts_with([' ', '\t', '#']) || line.trim().is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_matches(['"', '\'']).to_string();
        if value.is_empty() {
            continue;
        }

        match key.trim() {
            "name" => {
                metadata.name = Some(value.clone());
                metadata.id = Some(value);
            }
            "version" => metadata.version = Some(value),
            "description" => metadata.description = Some(value),
            "author" => metadata.authors = vec![value],
            "authors" => {
                metadata.authors = value
                    .trim_matches(['[', ']'])
                    .split(',')
                    .map(|author| author.trim().trim_matches(['"', '\'']).to_string())
                    .filter(|author| !author.is_empty())
                    .collect()
            }
            "api-version" => metadata.game_versions = vec![value],
            _ => {}
        }
    }

    Ok(metadata)
}

/// Reads whichever metadata file a jar carries. Blocking: callers wrap it.
pub fn read_jar(path: &Path) -> AppResult<Option<JarMetadata>> {
    let file = std::fs::File::open(path)
        .map_err(|e| AppError::io("open jar", path, e))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| AppError::Other(format!("{} is not a readable jar: {e}", path.display())))?;

    let read_entry = |zip: &mut zip::ZipArchive<std::fs::File>, name: &str| -> Option<String> {
        let mut entry = zip.by_name(name).ok()?;
        let mut body = String::new();
        entry.read_to_string(&mut body).ok()?;
        Some(body)
    };

    // NeoForge first: a jar that ships both is a NeoForge jar with a legacy
    // file kept for compatibility.
    if let Some(body) = read_entry(&mut zip, "META-INF/neoforge.mods.toml") {
        return parse_mods_toml(&body, true).map(Some);
    }
    if let Some(body) = read_entry(&mut zip, "fabric.mod.json") {
        return parse_fabric(&body).map(Some);
    }
    if let Some(body) = read_entry(&mut zip, "META-INF/mods.toml") {
        return parse_mods_toml(&body, false).map(Some);
    }
    if let Some(body) = read_entry(&mut zip, "paper-plugin.yml") {
        return parse_plugin_yml(&body, true).map(Some);
    }
    if let Some(body) = read_entry(&mut zip, "plugin.yml") {
        return parse_plugin_yml(&body, false).map(Some);
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_fabric_metadata_including_the_minecraft_dependency() {
        let body = r#"{
            "schemaVersion": 1,
            "id": "sodium",
            "name": "Sodium",
            "version": "0.6.9",
            "description": "A modern rendering engine",
            "authors": ["JellySquid", {"name": "Contributors"}],
            "depends": {"minecraft": "1.21.4", "fabricloader": ">=0.15.0"}
        }"#;

        let metadata = parse_fabric(body).unwrap();
        assert_eq!(metadata.format, "fabric.mod.json");
        assert_eq!(metadata.id.as_deref(), Some("sodium"));
        assert_eq!(metadata.name.as_deref(), Some("Sodium"));
        assert_eq!(metadata.version.as_deref(), Some("0.6.9"));
        assert_eq!(metadata.loaders, vec!["fabric"]);
        assert_eq!(metadata.game_versions, vec!["1.21.4"]);
        assert_eq!(metadata.authors, vec!["JellySquid", "Contributors"]);
    }

    #[test]
    fn a_fabric_version_list_is_kept_whole() {
        let body = r#"{"id":"x","depends":{"minecraft":["1.21.3","1.21.4"]}}"#;
        assert_eq!(
            parse_fabric(body).unwrap().game_versions,
            vec!["1.21.3", "1.21.4"]
        );
    }

    const FORGE_TOML: &str = r#"
modLoader = "javafml"
loaderVersion = "[47,)"
license = "MIT"

[[mods]]
modId = "jei"
version = "15.2.0.27"
displayName = "Just Enough Items"
authors = "mezz"
description = '''
An item and recipe viewer.
'''

[[dependencies.jei]]
modId = "minecraft"
mandatory = true
versionRange = "[1.20.1,1.21)"
side = "BOTH"
"#;

    #[test]
    fn reads_forge_metadata() {
        let metadata = parse_mods_toml(FORGE_TOML, false).unwrap();
        assert_eq!(metadata.format, "META-INF/mods.toml");
        assert_eq!(metadata.id.as_deref(), Some("jei"));
        assert_eq!(metadata.name.as_deref(), Some("Just Enough Items"));
        assert_eq!(metadata.version.as_deref(), Some("15.2.0.27"));
        assert_eq!(metadata.loaders, vec!["forge"]);
        assert_eq!(metadata.game_versions, vec!["[1.20.1,1.21)"]);
        assert_eq!(metadata.authors, vec!["mezz"]);
    }

    #[test]
    fn reads_neoforge_metadata_from_its_own_file_name() {
        let metadata = parse_mods_toml(FORGE_TOML, true).unwrap();
        assert_eq!(metadata.format, "META-INF/neoforge.mods.toml");
        assert_eq!(metadata.loaders, vec!["neoforge"]);
        assert_eq!(metadata.id.as_deref(), Some("jei"));
    }

    #[test]
    fn reads_bukkit_plugin_metadata() {
        let body = "name: EssentialsX\nversion: 2.20.1\nmain: com.earth2me.essentials.Essentials\napi-version: 1.20\nauthors: [Zenexer, md_5]\ncommands:\n  home:\n    description: Teleport home\n";
        let metadata = parse_plugin_yml(body, false).unwrap();

        assert_eq!(metadata.format, "plugin.yml");
        assert_eq!(metadata.name.as_deref(), Some("EssentialsX"));
        assert_eq!(metadata.version.as_deref(), Some("2.20.1"));
        assert_eq!(metadata.loaders, vec!["paper"]);
        assert_eq!(metadata.game_versions, vec!["1.20"]);
        assert_eq!(metadata.authors, vec!["Zenexer", "md_5"]);
    }

    #[test]
    fn nested_yaml_keys_are_not_mistaken_for_top_level_ones() {
        // "description" under a command must not become the plugin description.
        let body = "name: Test\ncommands:\n  home:\n    description: Teleport home\n";
        let metadata = parse_plugin_yml(body, true).unwrap();
        assert_eq!(metadata.format, "paper-plugin.yml");
        assert_eq!(metadata.description, None);
        assert_eq!(metadata.name.as_deref(), Some("Test"));
    }

    #[test]
    fn version_ranges_are_understood_well_enough_to_stop_false_warnings() {
        assert!(version_matches("1.21.4", "1.21.4"));
        assert!(version_matches("*", "1.21.4"));
        assert!(version_matches("[1.20.1,1.22)", "1.21.4"));
        assert!(version_matches("[1.21.4,)", "1.21.4"));
        assert!(version_matches(">=1.20", "1.21.4"));

        assert!(!version_matches("[1.20.1,1.21)", "1.21.4"));
        assert!(!version_matches("1.20.1", "1.21.4"));
        assert!(!version_matches(">=1.22", "1.21.4"));
    }

    #[test]
    fn a_matching_jar_reports_no_mismatch() {
        let metadata = parse_fabric(r#"{"id":"x","depends":{"minecraft":"1.21.4"}}"#).unwrap();
        assert!(check(&metadata, Loader::Fabric, "1.21.4").is_empty());
    }

    #[test]
    fn a_wrong_loader_warns_rather_than_refusing() {
        let metadata = parse_fabric(r#"{"id":"x"}"#).unwrap();
        let mismatch = check(&metadata, Loader::Paper, "1.21.4");
        assert!(mismatch.loader.unwrap().contains("fabric"));
        // Nothing here rejects the install; the caller shows the warning.
    }

    #[test]
    fn a_wrong_game_version_warns_with_both_numbers() {
        let metadata = parse_fabric(r#"{"id":"x","depends":{"minecraft":"1.20.1"}}"#).unwrap();
        let mismatch = check(&metadata, Loader::Fabric, "1.21.4");
        let message = mismatch.game_version.unwrap();
        assert!(message.contains("1.20.1") && message.contains("1.21.4"));
    }

    #[test]
    fn neoforge_accepts_forge_jars_but_not_the_other_way_round() {
        let forge_jar = parse_mods_toml(FORGE_TOML, false).unwrap();
        assert!(check(&forge_jar, Loader::NeoForge, "1.20.4").loader.is_none());

        let neoforge_jar = parse_mods_toml(FORGE_TOML, true).unwrap();
        assert!(check(&neoforge_jar, Loader::Forge, "1.20.4").loader.is_some());
    }

    #[test]
    fn a_jar_that_declares_nothing_produces_no_warnings() {
        let metadata = JarMetadata::empty("plugin.yml");
        assert!(check(&metadata, Loader::Fabric, "1.21.4").is_empty());
    }

    #[test]
    fn reads_metadata_out_of_a_real_jar_file() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let jar = dir.path().join("example.jar");
        {
            let file = std::fs::File::create(&jar).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            zip.start_file("fabric.mod.json", options).unwrap();
            zip.write_all(br#"{"id":"example","name":"Example","version":"1.0.0"}"#)
                .unwrap();
            zip.finish().unwrap();
        }

        let metadata = read_jar(&jar).unwrap().expect("metadata");
        assert_eq!(metadata.id.as_deref(), Some("example"));
        assert_eq!(metadata.format, "fabric.mod.json");
    }

    #[test]
    fn a_jar_with_both_forge_files_reads_as_neoforge() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let jar = dir.path().join("both.jar");
        {
            let file = std::fs::File::create(&jar).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            zip.start_file("META-INF/mods.toml", options).unwrap();
            zip.write_all(FORGE_TOML.as_bytes()).unwrap();
            zip.start_file("META-INF/neoforge.mods.toml", options).unwrap();
            zip.write_all(FORGE_TOML.as_bytes()).unwrap();
            zip.finish().unwrap();
        }

        let metadata = read_jar(&jar).unwrap().unwrap();
        assert_eq!(metadata.format, "META-INF/neoforge.mods.toml");
        assert_eq!(metadata.loaders, vec!["neoforge"]);
    }

    #[test]
    fn a_jar_without_metadata_is_not_an_error() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let jar = dir.path().join("plain.jar");
        {
            let file = std::fs::File::create(&jar).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            zip.start_file("README.txt", options).unwrap();
            zip.write_all(b"nothing to declare").unwrap();
            zip.finish().unwrap();
        }

        assert!(read_jar(&jar).unwrap().is_none());
    }

    #[test]
    fn something_that_is_not_a_jar_is_reported_clearly() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("not.jar");
        std::fs::write(&fake, b"just text").unwrap();
        assert!(read_jar(&fake).unwrap_err().to_string().contains("not a readable jar"));
    }
}
