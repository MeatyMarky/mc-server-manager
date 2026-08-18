//! Forge, via its promotions file plus the Maven repository.
//!
//! `promotions_slim.json` names the `latest` and `recommended` Forge version per
//! Minecraft version; the Maven metadata lists every build. Forge always ships
//! an installer that has to be run — there is no directly runnable server jar
//! for modern versions.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::error::{AppError, AppResult};
use crate::http::Fetch;
use crate::mcversion::{self, VersionIndex};

use super::{parse_maven_versions, Artifact, ArtifactKind, BuildEntry, VersionEntry};

pub const PROMOTIONS_URL: &str =
    "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json";
pub const MAVEN_ROOT: &str = "https://maven.minecraftforge.net/net/minecraftforge/forge";

pub fn metadata_url() -> String {
    format!("{MAVEN_ROOT}/maven-metadata.xml")
}

/// Maven coordinates are `<mc>-<forge>`, and legacy entries can carry a further
/// suffix (`1.7.10-10.13.4.1614-1.7.10`), so the full coordinate is kept intact.
pub fn installer_url(coordinate: &str) -> String {
    format!("{MAVEN_ROOT}/{coordinate}/forge-{coordinate}-installer.jar")
}

#[derive(Debug, Deserialize)]
struct Promotions {
    promos: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Promotion {
    pub mc_version: String,
    pub forge_version: String,
    pub recommended: bool,
}

/// Flattens `{"1.21.4-latest": "54.1.6"}` into structured promotions.
pub fn parse_promotions(body: &str) -> AppResult<Vec<Promotion>> {
    let promotions: Promotions = serde_json::from_str(body)?;
    let mut out = Vec::new();
    for (key, forge_version) in promotions.promos {
        let Some((mc_version, channel)) = key.rsplit_once('-') else {
            continue;
        };
        if mcversion::parse(mc_version).is_none() {
            continue;
        }
        out.push(Promotion {
            mc_version: mc_version.to_string(),
            forge_version,
            recommended: channel == "recommended",
        });
    }
    Ok(out)
}

/// Minecraft versions Forge publishes for. Ordering is applied by the caller
/// with the release chronology.
pub fn versions_from_promotions(body: &str) -> AppResult<Vec<VersionEntry>> {
    let promotions = parse_promotions(body)?;
    let mut ids: Vec<String> = Vec::new();
    for promotion in promotions {
        if !ids.contains(&promotion.mc_version) {
            ids.push(promotion.mc_version);
        }
    }
    Ok(ids
        .into_iter()
        .map(|id| VersionEntry { id, stable: true })
        .collect())
}

/// Maven coordinates for one Minecraft version, newest first, with the
/// recommended build marked.
pub fn builds_for(xml: &str, promotions: &[Promotion], mc_version: &str) -> Vec<BuildEntry> {
    let recommended = promotions
        .iter()
        .find(|p| p.mc_version == mc_version && p.recommended)
        .map(|p| format!("{mc_version}-{}", p.forge_version));

    let prefix = format!("{mc_version}-");
    let mut builds: Vec<BuildEntry> = parse_maven_versions(xml)
        .into_iter()
        .filter(|v| v.starts_with(&prefix))
        .map(|v| BuildEntry {
            stable: Some(&v) == recommended.as_ref(),
            label: (Some(&v) == recommended.as_ref()).then(|| "recommended".to_string()),
            id: v,
        })
        .collect();
    builds.reverse();
    builds
}

pub async fn list_versions<F: Fetch>(
    fetch: &F,
    index: &VersionIndex,
) -> AppResult<Vec<VersionEntry>> {
    let versions = versions_from_promotions(&fetch.get_text(PROMOTIONS_URL).await?)?;
    Ok(super::sort_entries(versions, index))
}

pub async fn list_builds<F: Fetch>(fetch: &F, mc_version: &str) -> AppResult<Vec<BuildEntry>> {
    let promotions = parse_promotions(&fetch.get_text(PROMOTIONS_URL).await?)?;
    let xml = fetch.get_text(&metadata_url()).await?;
    Ok(builds_for(&xml, &promotions, mc_version))
}

/// `build` is a full Maven coordinate (`1.21.4-54.1.6`) or a bare Forge version
/// (`54.1.6`); both are accepted because the UI shows the former and users
/// quote the latter.
pub async fn resolve<F: Fetch>(
    fetch: &F,
    mc_version: &str,
    build: Option<&str>,
) -> AppResult<Artifact> {
    let promotions = parse_promotions(&fetch.get_text(PROMOTIONS_URL).await?)?;

    let coordinate = match build {
        Some(wanted) => {
            let candidate = if wanted.starts_with(&format!("{mc_version}-")) {
                wanted.to_string()
            } else {
                format!("{mc_version}-{wanted}")
            };
            let xml = fetch.get_text(&metadata_url()).await?;
            if !parse_maven_versions(&xml).iter().any(|v| v == &candidate) {
                return Err(AppError::VersionNotFound {
                    kind: "Forge",
                    version: candidate,
                });
            }
            candidate
        }
        None => {
            // Recommended when there is one, otherwise latest.
            let pick = promotions
                .iter()
                .find(|p| p.mc_version == mc_version && p.recommended)
                .or_else(|| promotions.iter().find(|p| p.mc_version == mc_version))
                .ok_or_else(|| AppError::VersionNotFound {
                    kind: "Forge",
                    version: mc_version.to_string(),
                })?;
            format!("{mc_version}-{}", pick.forge_version)
        }
    };

    Ok(Artifact {
        url: installer_url(&coordinate),
        file_name: format!("forge-{coordinate}-installer.jar"),
        kind: ArtifactKind::Installer,
        sha1: None,
        sha256: None,
        md5: None,
        size: None,
        build: Some(coordinate),
        java_major: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::FixtureFetch;

    fn fixtures() -> FixtureFetch {
        FixtureFetch::new()
            .route(PROMOTIONS_URL, "forge_promotions_slim.json")
            .route(&metadata_url(), "forge_maven_metadata.xml")
    }

    fn promotions() -> Vec<Promotion> {
        let body = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/forge_promotions_slim.json"),
        )
        .unwrap();
        parse_promotions(&body).unwrap()
    }

    #[test]
    fn promotions_split_into_version_and_channel() {
        let promotions = promotions();
        let recommended: Vec<&Promotion> = promotions.iter().filter(|p| p.recommended).collect();
        assert!(!recommended.is_empty());
        assert!(promotions.iter().any(|p| p.mc_version == "1.21.4"));
        assert!(promotions.iter().any(|p| p.mc_version == "26.2"));
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
    async fn versions_are_newest_first_and_cover_the_calendar_era() {
        let versions = list_versions(&fixtures(), &index()).await.unwrap();
        assert_eq!(versions.first().map(|v| v.id.as_str()), Some("26.2"));
    }

    #[tokio::test]
    async fn resolve_prefers_the_recommended_build() {
        let artifact = resolve(&fixtures(), "26.2", None).await.unwrap();
        assert_eq!(artifact.kind, ArtifactKind::Installer);
        assert_eq!(artifact.build.as_deref(), Some("26.2-65.1.0"));
        assert_eq!(
            artifact.url,
            format!("{MAVEN_ROOT}/26.2-65.1.0/forge-26.2-65.1.0-installer.jar")
        );
    }

    #[tokio::test]
    async fn a_bare_forge_version_is_accepted() {
        // Taken from the fixture rather than hardcoded: refreshing fixtures
        // changes which builds exist, and that must not break this assertion.
        let builds = list_builds(&fixtures(), "1.21.4").await.unwrap();
        let coordinate = builds.first().expect("a 1.21.4 build in the fixture").id.clone();
        let bare = coordinate.trim_start_matches("1.21.4-").to_string();

        let artifact = resolve(&fixtures(), "1.21.4", Some(&bare)).await.unwrap();
        assert_eq!(artifact.build.as_deref(), Some(coordinate.as_str()));
    }

    #[tokio::test]
    async fn a_build_that_is_not_published_is_rejected() {
        let err = resolve(&fixtures(), "1.21.4", Some("99.9.9")).await.unwrap_err();
        assert_eq!(err.kind(), "version_not_found");
    }

    #[tokio::test]
    async fn builds_mark_the_recommended_coordinate() {
        let builds = list_builds(&fixtures(), "26.2").await.unwrap();
        assert!(builds.iter().any(|b| b.stable && b.id == "26.2-65.1.0"));
        assert!(builds.iter().all(|b| b.id.starts_with("26.2-")));
    }

    #[test]
    fn legacy_coordinates_keep_their_suffix() {
        assert_eq!(
            installer_url("1.7.10-10.13.4.1614-1.7.10"),
            format!("{MAVEN_ROOT}/1.7.10-10.13.4.1614-1.7.10/forge-1.7.10-10.13.4.1614-1.7.10-installer.jar")
        );
    }
}
