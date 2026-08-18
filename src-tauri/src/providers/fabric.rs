//! Fabric, via `meta.fabricmc.net/v2`.
//!
//! Fabric's meta service builds the server launcher on demand at
//! `/versions/loader/<game>/<loader>/<installer>/server/jar`, so resolution is
//! three list lookups and a URL. No checksum is published for that endpoint.

use serde::Deserialize;

use crate::error::{AppError, AppResult};
use crate::http::Fetch;
use crate::mcversion::VersionIndex;

use super::{Artifact, ArtifactKind, BuildEntry, VersionEntry};

pub const GAME_URL: &str = "https://meta.fabricmc.net/v2/versions/game";
pub const LOADER_URL: &str = "https://meta.fabricmc.net/v2/versions/loader";
pub const INSTALLER_URL: &str = "https://meta.fabricmc.net/v2/versions/installer";

pub fn server_jar_url(mc_version: &str, loader: &str, installer: &str) -> String {
    format!(
        "https://meta.fabricmc.net/v2/versions/loader/{mc_version}/{loader}/{installer}/server/jar"
    )
}

#[derive(Debug, Deserialize)]
struct GameVersion {
    version: String,
    stable: bool,
}

#[derive(Debug, Deserialize)]
struct LoaderVersion {
    version: String,
    stable: bool,
}

#[derive(Debug, Deserialize)]
struct InstallerVersion {
    version: String,
    stable: bool,
}

pub fn parse_game_versions(body: &str) -> AppResult<Vec<VersionEntry>> {
    let versions: Vec<GameVersion> = serde_json::from_str(body)?;
    Ok(versions
        .into_iter()
        .filter(|v| v.stable)
        .map(|v| VersionEntry {
            id: v.version,
            stable: true,
        })
        .collect())
}

pub fn parse_loaders(body: &str) -> AppResult<Vec<BuildEntry>> {
    let versions: Vec<LoaderVersion> = serde_json::from_str(body)?;
    Ok(versions
        .into_iter()
        .map(|v| BuildEntry {
            label: v.stable.then(|| "stable".to_string()),
            id: v.version,
            stable: v.stable,
        })
        .collect())
}

/// Newest stable installer, which is what the launcher endpoint expects.
fn newest_stable_installer(body: &str) -> AppResult<String> {
    let versions: Vec<InstallerVersion> = serde_json::from_str(body)?;
    versions
        .iter()
        .find(|v| v.stable)
        .or_else(|| versions.first())
        .map(|v| v.version.clone())
        .ok_or_else(|| AppError::Network("Fabric published no installer versions".into()))
}

pub async fn list_versions<F: Fetch>(
    fetch: &F,
    index: &VersionIndex,
) -> AppResult<Vec<VersionEntry>> {
    let versions = parse_game_versions(&fetch.get_text(GAME_URL).await?)?;
    Ok(super::sort_entries(versions, index))
}

pub async fn list_builds<F: Fetch>(fetch: &F, _mc_version: &str) -> AppResult<Vec<BuildEntry>> {
    parse_loaders(&fetch.get_text(LOADER_URL).await?)
}

pub async fn resolve<F: Fetch>(
    fetch: &F,
    mc_version: &str,
    build: Option<&str>,
) -> AppResult<Artifact> {
    let loaders = parse_loaders(&fetch.get_text(LOADER_URL).await?)?;
    let loader = match build {
        Some(wanted) => loaders
            .iter()
            .find(|l| l.id == wanted)
            .map(|l| l.id.clone())
            .ok_or_else(|| AppError::VersionNotFound {
                kind: "Fabric loader",
                version: wanted.to_string(),
            })?,
        None => loaders
            .iter()
            .find(|l| l.stable)
            .or_else(|| loaders.first())
            .map(|l| l.id.clone())
            .ok_or_else(|| AppError::VersionNotFound {
                kind: "Fabric loader",
                version: mc_version.to_string(),
            })?,
    };

    let installer = newest_stable_installer(&fetch.get_text(INSTALLER_URL).await?)?;

    Ok(Artifact {
        url: server_jar_url(mc_version, &loader, &installer),
        file_name: format!("fabric-server-mc.{mc_version}-loader.{loader}-launcher.{installer}.jar"),
        kind: ArtifactKind::ServerJar,
        sha1: None,
        sha256: None,
        sha512: None,
        md5: None,
        size: None,
        build: Some(loader),
        java_major: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::FixtureFetch;

    fn fixtures() -> FixtureFetch {
        FixtureFetch::new()
            .route(GAME_URL, "fabric_game.json")
            .route(LOADER_URL, "fabric_loader.json")
            .route(INSTALLER_URL, "fabric_installer.json")
    }

    fn index() -> VersionIndex {
        let body = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/vanilla_version_manifest_v2.json"),
        )
        .unwrap();
        VersionIndex::from_entries(
            crate::providers::vanilla::parse_manifest_entries(&body).unwrap(),
        )
    }

    #[tokio::test]
    async fn only_stable_game_versions_are_offered() {
        let versions = list_versions(&fixtures(), &index()).await.unwrap();
        let ids: Vec<&str> = versions.iter().map(|v| v.id.as_str()).collect();
        assert!(ids.contains(&"26.2"));
        assert!(!ids.iter().any(|id| id.contains("snapshot")));
    }

    #[tokio::test]
    async fn resolve_builds_the_launcher_url_from_game_loader_and_installer() {
        let artifact = resolve(&fixtures(), "1.21.4", None).await.unwrap();
        assert!(artifact.url.starts_with("https://meta.fabricmc.net/v2/versions/loader/1.21.4/"));
        assert!(artifact.url.ends_with("/server/jar"));
        assert_eq!(artifact.kind, ArtifactKind::ServerJar);
        // Fabric publishes no checksum for the generated launcher.
        assert!(artifact.sha1.is_none() && artifact.sha256.is_none());
        assert!(artifact.file_name.starts_with("fabric-server-mc.1.21.4-loader."));
    }

    #[tokio::test]
    async fn a_pinned_loader_is_honoured() {
        let artifact = resolve(&fixtures(), "1.21.4", Some("0.19.2")).await.unwrap();
        assert_eq!(artifact.build.as_deref(), Some("0.19.2"));
        assert!(artifact.url.contains("/1.21.4/0.19.2/"));
    }

    #[tokio::test]
    async fn an_unknown_loader_is_rejected() {
        let err = resolve(&fixtures(), "1.21.4", Some("0.0.0")).await.unwrap_err();
        assert_eq!(err.kind(), "version_not_found");
    }

    #[test]
    fn the_url_shape_matches_fabrics_documented_endpoint() {
        assert_eq!(
            server_jar_url("1.20.1", "0.15.7", "1.0.1"),
            "https://meta.fabricmc.net/v2/versions/loader/1.20.1/0.15.7/1.0.1/server/jar"
        );
    }
}
