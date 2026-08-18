//! Version resolution and jar/installer URL building for the six server types.
//!
//! Everything here is generic over [`Fetch`], so every resolution path is
//! exercised in tests against recorded API payloads under `tests/fixtures/`.
//! No test touches the network.

pub mod fabric;
pub mod forge;
pub mod neoforge;
pub mod paper;
pub mod purpur;
pub mod vanilla;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::db::models::ServerType;
use crate::error::AppResult;
use crate::http::Fetch;

/// What gets downloaded: either the server jar itself, or an installer that has
/// to be run to produce a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub enum ArtifactKind {
    ServerJar,
    Installer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct Artifact {
    pub url: String,
    pub file_name: String,
    pub kind: ArtifactKind,
    pub sha1: Option<String>,
    pub sha256: Option<String>,
    pub md5: Option<String>,
    #[ts(type = "number | null")]
    pub size: Option<u64>,
    /// The build/loader version this artifact corresponds to, when the provider
    /// has one (Paper build number, Fabric loader version, Forge version).
    pub build: Option<String>,
    /// Java major version the server requires, when the provider states it.
    #[ts(type = "number | null")]
    pub java_major: Option<i64>,
}

/// One selectable Minecraft version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct VersionEntry {
    pub id: String,
    pub stable: bool,
}

/// One selectable build for a given Minecraft version (Paper build, Forge
/// version, Fabric loader, …). Empty for Vanilla.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct BuildEntry {
    pub id: String,
    pub stable: bool,
    pub label: Option<String>,
}

/// Minecraft versions this server type offers, newest first.
pub async fn list_versions<F: Fetch>(
    server_type: ServerType,
    fetch: &F,
) -> AppResult<Vec<VersionEntry>> {
    match server_type {
        ServerType::Vanilla => vanilla::list_versions(fetch).await,
        ServerType::Paper => paper::list_versions(fetch).await,
        ServerType::Purpur => purpur::list_versions(fetch).await,
        ServerType::Fabric => fabric::list_versions(fetch).await,
        ServerType::Forge => forge::list_versions(fetch).await,
        ServerType::NeoForge => neoforge::list_versions(fetch).await,
    }
}

/// Builds available for one Minecraft version, newest first.
pub async fn list_builds<F: Fetch>(
    server_type: ServerType,
    fetch: &F,
    mc_version: &str,
) -> AppResult<Vec<BuildEntry>> {
    match server_type {
        ServerType::Vanilla => Ok(Vec::new()),
        ServerType::Paper => paper::list_builds(fetch, mc_version).await,
        ServerType::Purpur => purpur::list_builds(fetch, mc_version).await,
        ServerType::Fabric => fabric::list_builds(fetch, mc_version).await,
        ServerType::Forge => forge::list_builds(fetch, mc_version).await,
        ServerType::NeoForge => neoforge::list_builds(fetch, mc_version).await,
    }
}

/// Resolves the exact artifact to download. `build` selects a specific build;
/// `None` means "newest stable".
pub async fn resolve<F: Fetch>(
    server_type: ServerType,
    fetch: &F,
    mc_version: &str,
    build: Option<&str>,
) -> AppResult<Artifact> {
    match server_type {
        ServerType::Vanilla => vanilla::resolve(fetch, mc_version).await,
        ServerType::Paper => paper::resolve(fetch, mc_version, build).await,
        ServerType::Purpur => purpur::resolve(fetch, mc_version, build).await,
        ServerType::Fabric => fabric::resolve(fetch, mc_version, build).await,
        ServerType::Forge => forge::resolve(fetch, mc_version, build).await,
        ServerType::NeoForge => neoforge::resolve(fetch, mc_version, build).await,
    }
}

/// Minimal `<version>` extraction from a Maven `maven-metadata.xml`. Both
/// modded providers publish one, and neither needs a full XML parser.
pub fn parse_maven_versions(xml: &str) -> Vec<String> {
    let mut versions = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<version>") {
        let after = &rest[start + "<version>".len()..];
        let Some(end) = after.find("</version>") else {
            break;
        };
        let value = after[..end].trim();
        if !value.is_empty() {
            versions.push(value.to_string());
        }
        rest = &after[end..];
    }
    versions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_maven_versions_without_an_xml_parser() {
        let xml = r#"<metadata><versioning><versions>
            <version>21.1.65</version>
            <version> 26.2.0.62 </version>
        </versions></versioning></metadata>"#;
        assert_eq!(parse_maven_versions(xml), vec!["21.1.65", "26.2.0.62"]);
    }

    #[test]
    fn tolerates_truncated_metadata() {
        assert!(parse_maven_versions("<versions><version>21.1.65").is_empty());
        assert!(parse_maven_versions("").is_empty());
    }
}
