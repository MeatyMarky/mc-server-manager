//! Paper, via the Fill API (`fill.papermc.io/v3`).
//!
//! The v2 API named in the original brief has been sunset — it now answers
//! `{"ok":false,"error":"sunset"}` for every request — so v3 is what this
//! provider talks to. Downloads carry a SHA-256, which is verified after every
//! transfer.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::error::{AppError, AppResult};
use crate::http::Fetch;
use crate::mcversion;

use super::{Artifact, ArtifactKind, BuildEntry, VersionEntry};

pub const PROJECT_URL: &str = "https://fill.papermc.io/v3/projects/paper";

pub fn builds_url(mc_version: &str) -> String {
    format!("{PROJECT_URL}/versions/{mc_version}/builds")
}

#[derive(Debug, Deserialize)]
struct Project {
    /// Family ("1.21") -> versions in that family, newest first.
    versions: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct Build {
    id: u64,
    channel: String,
    downloads: BTreeMap<String, BuildDownload>,
}

#[derive(Debug, Deserialize)]
struct BuildDownload {
    name: String,
    url: String,
    size: Option<u64>,
    checksums: BTreeMap<String, String>,
}

pub fn parse_versions(body: &str) -> AppResult<Vec<VersionEntry>> {
    let project: Project = serde_json::from_str(body)?;
    let mut ids: Vec<String> = project.versions.into_values().flatten().collect();
    ids.retain(|id| mcversion::parse(id).is_some_and(|v| v.is_release()));
    mcversion::sort_newest_first(&mut ids);
    Ok(ids
        .into_iter()
        .map(|id| VersionEntry { id, stable: true })
        .collect())
}

fn parse_builds(body: &str) -> AppResult<Vec<Build>> {
    let mut builds: Vec<Build> = serde_json::from_str(body)?;
    builds.sort_by_key(|build| std::cmp::Reverse(build.id));
    Ok(builds)
}

pub fn parse_build_entries(body: &str) -> AppResult<Vec<BuildEntry>> {
    Ok(parse_builds(body)?
        .into_iter()
        .map(|build| BuildEntry {
            stable: build.channel.eq_ignore_ascii_case("STABLE"),
            label: Some(build.channel.to_ascii_lowercase()),
            id: build.id.to_string(),
        })
        .collect())
}

/// `server:default` is the plain server jar; other keys are mojmap variants we
/// have no use for.
fn artifact_from_build(build: Build, mc_version: &str) -> AppResult<Artifact> {
    let download = build
        .downloads
        .get("server:default")
        .ok_or_else(|| AppError::VersionNotFound {
            kind: "Paper server jar",
            version: mc_version.to_string(),
        })?;

    Ok(Artifact {
        url: download.url.clone(),
        file_name: download.name.clone(),
        kind: ArtifactKind::ServerJar,
        sha1: None,
        sha256: download.checksums.get("sha256").cloned(),
        md5: None,
        size: download.size,
        build: Some(build.id.to_string()),
        java_major: None,
    })
}

pub async fn list_versions<F: Fetch>(fetch: &F) -> AppResult<Vec<VersionEntry>> {
    parse_versions(&fetch.get_text(PROJECT_URL).await?)
}

pub async fn list_builds<F: Fetch>(fetch: &F, mc_version: &str) -> AppResult<Vec<BuildEntry>> {
    parse_build_entries(&fetch.get_text(&builds_url(mc_version)).await?)
}

/// `build = None` picks the newest stable build, falling back to the newest
/// build of any channel when a version has no stable one yet.
pub async fn resolve<F: Fetch>(
    fetch: &F,
    mc_version: &str,
    build: Option<&str>,
) -> AppResult<Artifact> {
    let body = fetch.get_text(&builds_url(mc_version)).await?;
    let builds = parse_builds(&body)?;

    let chosen = match build {
        Some(wanted) => builds.into_iter().find(|b| b.id.to_string() == wanted),
        None => match builds
            .iter()
            .position(|b| b.channel.eq_ignore_ascii_case("STABLE"))
        {
            Some(index) => builds.into_iter().nth(index),
            None => builds.into_iter().next(),
        },
    };

    let chosen = chosen.ok_or_else(|| AppError::VersionNotFound {
        kind: "Paper",
        version: match build {
            Some(b) => format!("{mc_version} build {b}"),
            None => mc_version.to_string(),
        },
    })?;
    artifact_from_build(chosen, mc_version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::FixtureFetch;

    fn fixtures() -> FixtureFetch {
        FixtureFetch::new()
            .route(PROJECT_URL, "paper_project.json")
            .route(&builds_url("1.21.4"), "paper_builds_1_21_4.json")
    }

    #[tokio::test]
    async fn versions_are_releases_newest_first_across_eras() {
        let versions = list_versions(&fixtures()).await.unwrap();
        let ids: Vec<&str> = versions.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids.first(), Some(&"26.2"));
        assert!(ids.contains(&"1.21.4"));
        // Release candidates and pre-releases are filtered out of the picker.
        assert!(!ids.iter().any(|id| id.contains("rc") || id.contains("pre")));
    }

    #[tokio::test]
    async fn resolve_picks_the_newest_build_and_keeps_its_sha256() {
        let artifact = resolve(&fixtures(), "1.21.4", None).await.unwrap();
        assert_eq!(artifact.kind, ArtifactKind::ServerJar);
        assert!(artifact.file_name.starts_with("paper-1.21.4-"));
        assert_eq!(artifact.sha256.as_ref().map(|s| s.len()), Some(64));
        assert!(artifact.url.contains(artifact.sha256.as_deref().unwrap()));
        assert!(artifact.size.unwrap() > 1_000_000);
    }

    #[tokio::test]
    async fn a_specific_build_can_be_pinned() {
        let builds = list_builds(&fixtures(), "1.21.4").await.unwrap();
        let oldest = builds.last().unwrap().id.clone();
        let artifact = resolve(&fixtures(), "1.21.4", Some(&oldest)).await.unwrap();
        assert_eq!(artifact.build.as_deref(), Some(oldest.as_str()));
    }

    #[tokio::test]
    async fn an_unknown_build_is_an_error_not_a_silent_fallback() {
        let err = resolve(&fixtures(), "1.21.4", Some("999999")).await.unwrap_err();
        assert_eq!(err.kind(), "version_not_found");
    }

    #[tokio::test]
    async fn builds_are_labelled_with_their_channel() {
        let builds = list_builds(&fixtures(), "1.21.4").await.unwrap();
        assert!(!builds.is_empty());
        assert!(builds.iter().all(|b| b.label.is_some()));
        assert!(builds.iter().any(|b| b.stable));
    }
}
