//! Purpur, via `api.purpurmc.org/v2`.
//!
//! The download endpoint streams the jar directly; the build metadata carries
//! an MD5, which is weak but is what the API publishes, so it is verified as a
//! transfer check rather than as a security guarantee.

use serde::Deserialize;

use crate::error::{AppError, AppResult};
use crate::http::Fetch;
use crate::mcversion::{self, VersionIndex};

use super::{Artifact, ArtifactKind, BuildEntry, VersionEntry};

pub const ROOT_URL: &str = "https://api.purpurmc.org/v2/purpur";

pub fn version_url(mc_version: &str) -> String {
    format!("{ROOT_URL}/{mc_version}")
}

pub fn build_url(mc_version: &str, build: &str) -> String {
    format!("{ROOT_URL}/{mc_version}/{build}")
}

/// Purpur serves the jar from the build endpoint plus `/download`.
pub fn download_url(mc_version: &str, build: &str) -> String {
    format!("{ROOT_URL}/{mc_version}/{build}/download")
}

#[derive(Debug, Deserialize)]
struct Root {
    versions: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct VersionDetail {
    builds: Builds,
}

#[derive(Debug, Deserialize)]
struct Builds {
    latest: String,
    all: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct BuildDetail {
    build: String,
    result: String,
    md5: Option<String>,
}

pub fn parse_versions(body: &str) -> AppResult<Vec<VersionEntry>> {
    let root: Root = serde_json::from_str(body)?;
    let mut ids = root.versions;
    ids.retain(|id| mcversion::parse(id).is_some_and(|v| v.is_release()));
    Ok(ids
        .into_iter()
        .map(|id| VersionEntry { id, stable: true })
        .collect())
}

/// Newest build first; the API lists them oldest first.
pub fn parse_builds(body: &str) -> AppResult<Vec<BuildEntry>> {
    let detail: VersionDetail = serde_json::from_str(body)?;
    let latest = detail.builds.latest;
    let mut builds: Vec<String> = detail.builds.all;
    builds.reverse();
    Ok(builds
        .into_iter()
        .map(|id| BuildEntry {
            stable: id == latest,
            label: (id == latest).then(|| "latest".to_string()),
            id,
        })
        .collect())
}

pub async fn list_versions<F: Fetch>(
    fetch: &F,
    index: &VersionIndex,
) -> AppResult<Vec<VersionEntry>> {
    let versions = parse_versions(&fetch.get_text(ROOT_URL).await?)?;
    Ok(super::sort_entries(versions, index))
}

pub async fn list_builds<F: Fetch>(fetch: &F, mc_version: &str) -> AppResult<Vec<BuildEntry>> {
    parse_builds(&fetch.get_text(&version_url(mc_version)).await?)
}

pub async fn resolve<F: Fetch>(
    fetch: &F,
    mc_version: &str,
    build: Option<&str>,
) -> AppResult<Artifact> {
    let wanted = match build {
        Some(build) => build.to_string(),
        None => {
            let detail: VersionDetail =
                serde_json::from_str(&fetch.get_text(&version_url(mc_version)).await?)?;
            detail.builds.latest
        }
    };

    let detail: BuildDetail =
        serde_json::from_str(&fetch.get_text(&build_url(mc_version, &wanted)).await?)?;
    if !detail.result.eq_ignore_ascii_case("SUCCESS") {
        return Err(AppError::VersionNotFound {
            kind: "successful Purpur build",
            version: format!("{mc_version} build {wanted}"),
        });
    }

    Ok(Artifact {
        url: download_url(mc_version, &detail.build),
        file_name: format!("purpur-{mc_version}-{}.jar", detail.build),
        kind: ArtifactKind::ServerJar,
        sha1: None,
        sha256: None,
        sha512: None,
        md5: detail.md5,
        size: None,
        build: Some(detail.build),
        java_major: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::FixtureFetch;

    fn fixtures() -> FixtureFetch {
        FixtureFetch::new()
            .route(ROOT_URL, "purpur_root.json")
            .route(&version_url("1.21.4"), "purpur_version_1_21_4.json")
            .route(&build_url("1.21.4", "2416"), "purpur_build_1_21_4.json")
    }

    #[tokio::test]
    async fn versions_are_sorted_by_release_chronology() {
        let body = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/vanilla_version_manifest_v2.json"),
        )
        .unwrap();
        let index = VersionIndex::from_entries(
            crate::providers::vanilla::parse_manifest_entries(&body).unwrap(),
        );
        let versions = list_versions(&fixtures(), &index).await.unwrap();
        let ids: Vec<&str> = versions.iter().map(|v| v.id.as_str()).collect();
        assert!(
            ids.windows(2).all(|w| !index.is_newer(w[1], w[0])),
            "not newest first: {ids:?}"
        );
    }

    #[tokio::test]
    async fn resolve_uses_the_latest_build_and_its_md5() {
        let artifact = resolve(&fixtures(), "1.21.4", None).await.unwrap();
        assert_eq!(artifact.build.as_deref(), Some("2416"));
        assert_eq!(artifact.file_name, "purpur-1.21.4-2416.jar");
        assert_eq!(
            artifact.url,
            "https://api.purpurmc.org/v2/purpur/1.21.4/2416/download"
        );
        assert_eq!(artifact.md5.as_ref().map(|m| m.len()), Some(32));
    }

    #[tokio::test]
    async fn builds_list_marks_the_latest_and_is_newest_first() {
        let builds = list_builds(&fixtures(), "1.21.4").await.unwrap();
        assert_eq!(builds.first().map(|b| b.id.as_str()), Some("2416"));
        assert!(builds.first().unwrap().stable);
        assert!(!builds.last().unwrap().stable);
    }

    #[test]
    fn urls_are_built_from_components_not_string_soup() {
        assert_eq!(
            download_url("26.2", "17"),
            "https://api.purpurmc.org/v2/purpur/26.2/17/download"
        );
    }
}
