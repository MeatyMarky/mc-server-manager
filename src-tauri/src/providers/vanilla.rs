//! Vanilla, via Mojang's version manifest.
//!
//! The per-version JSON also states `javaVersion.majorVersion`, which is the
//! authoritative Java requirement — better than any table we could hardcode,
//! and the reason 26.x correctly asks for Java 25.

use serde::Deserialize;

use crate::error::{AppError, AppResult};
use crate::http::Fetch;
use crate::mcversion::{IndexedVersion, VersionIndex};

use super::{Artifact, ArtifactKind, BuildEntry, VersionEntry};

pub const MANIFEST_URL: &str =
    "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json";

#[derive(Debug, Deserialize)]
struct Manifest {
    versions: Vec<ManifestVersion>,
}

#[derive(Debug, Deserialize)]
struct ManifestVersion {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    url: String,
    #[serde(rename = "releaseTime")]
    release_time: String,
}

#[derive(Debug, Deserialize)]
struct VersionDetail {
    #[serde(rename = "javaVersion")]
    java_version: Option<JavaVersion>,
    downloads: Downloads,
}

#[derive(Debug, Deserialize)]
struct JavaVersion {
    #[serde(rename = "majorVersion")]
    major_version: i64,
}

#[derive(Debug, Deserialize)]
struct Downloads {
    server: Option<Download>,
}

#[derive(Debug, Deserialize)]
struct Download {
    sha1: String,
    size: u64,
    url: String,
}

/// The whole manifest as chronology entries. Manifest order is newest first,
/// and that order plus `releaseTime` is what every version sort relies on.
pub fn parse_manifest_entries(body: &str) -> AppResult<Vec<IndexedVersion>> {
    let manifest: Manifest = serde_json::from_str(body)?;
    Ok(manifest
        .versions
        .into_iter()
        .enumerate()
        .map(|(position, version)| IndexedVersion {
            id: version.id,
            release_time: version.release_time,
            kind: version.kind,
            position: position as i64,
        })
        .collect())
}

/// Every version with a server to run, newest first.
///
/// Snapshots and pre-releases are kept - the picker filters them, and someone
/// testing a mod against next week's snapshot needs to be able to choose one.
/// The alpha and beta builds are dropped: Mojang published no server jar before
/// 1.2.5, so offering them is offering a failure.
pub fn parse_manifest(body: &str) -> AppResult<Vec<VersionEntry>> {
    let manifest: Manifest = serde_json::from_str(body)?;
    Ok(manifest
        .versions
        .into_iter()
        .filter(|v| v.kind != "old_alpha" && v.kind != "old_beta")
        .map(|v| {
            let kind = crate::mcversion::classify_kind(&v.id, &v.kind);
            VersionEntry {
                stable: kind == crate::mcversion::VersionKind::Release,
                release_time: Some(v.release_time),
                kind,
                id: v.id,
            }
        })
        .collect())
}

fn find_version_url(body: &str, mc_version: &str) -> AppResult<String> {
    let manifest: Manifest = serde_json::from_str(body)?;
    manifest
        .versions
        .into_iter()
        .find(|v| v.id == mc_version)
        .map(|v| v.url)
        .ok_or_else(|| AppError::VersionNotFound {
            kind: "Vanilla",
            version: mc_version.to_string(),
        })
}

/// Pulls the server download out of a per-version JSON. Versions before 1.2.5
/// have no server download at all, which is reported as such.
pub fn parse_version_detail(body: &str, mc_version: &str) -> AppResult<Artifact> {
    let detail: VersionDetail = serde_json::from_str(body)?;
    let server = detail.downloads.server.ok_or_else(|| AppError::VersionNotFound {
        kind: "Vanilla server jar",
        version: mc_version.to_string(),
    })?;

    Ok(Artifact {
        url: server.url,
        file_name: "server.jar".to_string(),
        kind: ArtifactKind::ServerJar,
        sha1: Some(server.sha1),
        sha256: None,
        sha512: None,
        md5: None,
        size: Some(server.size),
        build: None,
        java_major: detail.java_version.map(|j| j.major_version),
    })
}

pub async fn list_versions<F: Fetch>(
    fetch: &F,
    index: &VersionIndex,
) -> AppResult<Vec<VersionEntry>> {
    let body = fetch.get_text(MANIFEST_URL).await?;
    let versions = parse_manifest(&body)?;
    Ok(super::sort_entries(versions, index))
}

pub async fn list_builds<F: Fetch>(_fetch: &F, _mc_version: &str) -> AppResult<Vec<BuildEntry>> {
    Ok(Vec::new())
}

pub async fn resolve<F: Fetch>(fetch: &F, mc_version: &str) -> AppResult<Artifact> {
    let manifest = fetch.get_text(MANIFEST_URL).await?;
    let url = find_version_url(&manifest, mc_version)?;
    let detail = fetch.get_text(&url).await?;
    parse_version_detail(&detail, mc_version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::FixtureFetch;

    fn fixtures() -> FixtureFetch {
        FixtureFetch::new()
            .route(MANIFEST_URL, "vanilla_version_manifest_v2.json")
            .route(
                "https://piston-meta.mojang.com/v1/packages/c75d82e7fa6eca5a043dab0c6cf77cb8317644f4/26.2.json",
                "vanilla_version_26_2.json",
            )
            .route(
                "https://piston-meta.mojang.com/v1/packages/7a6a540b8b43659d4959beec59b8b1e79fec81c6/1.21.4.json",
                "vanilla_version_1_21_4.json",
            )
    }

    /// The picker filters by kind, so the list has to carry every kind that
    /// has a server jar - and mark which is which.
    #[test]
    fn manifest_keeps_snapshots_and_labels_every_kind() {
        let body = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/vanilla_version_manifest_v2.json"),
        )
        .unwrap();
        let versions = parse_manifest(&body).unwrap();
        let ids: Vec<&str> = versions.iter().map(|v| v.id.as_str()).collect();
        assert!(ids.contains(&"26.2"));
        assert!(ids.contains(&"1.21.4"));
        assert!(ids.iter().any(|id| id.contains("snapshot")));

        let release = versions.iter().find(|v| v.id == "26.2").unwrap();
        assert_eq!(release.kind, crate::mcversion::VersionKind::Release);
        assert!(release.stable);
        // A date on every row: the table's whole reason for existing.
        assert!(release.release_time.as_deref().unwrap().contains('T'));

        let snapshot = versions.iter().find(|v| v.id.contains("snapshot")).unwrap();
        assert_eq!(snapshot.kind, crate::mcversion::VersionKind::Snapshot);
        assert!(!snapshot.stable);
    }

    /// Alpha and beta predate the server jar entirely, so offering them would
    /// only ever produce a failed install.
    #[test]
    fn the_pre_server_era_is_left_out() {
        let body = manifest_body();
        let versions = parse_manifest(&body).unwrap();
        assert!(!versions
            .iter()
            .any(|v| v.kind == crate::mcversion::VersionKind::Ancient));
    }

    #[test]
    fn version_detail_yields_url_sha1_and_java_requirement() {
        let body = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/vanilla_version_26_2.json"),
        )
        .unwrap();
        let artifact = parse_version_detail(&body, "26.2").unwrap();
        assert_eq!(artifact.kind, ArtifactKind::ServerJar);
        assert_eq!(artifact.file_name, "server.jar");
        assert!(artifact.url.starts_with("https://"));
        assert_eq!(artifact.sha1.as_deref().map(str::len), Some(40));
        // The calendar era needs Java 25; a hardcoded 1.20.5+ -> 21 table would lie.
        assert_eq!(artifact.java_major, Some(25));
    }

    #[test]
    fn older_versions_report_their_own_java_requirement() {
        for (fixture, expected) in [
            ("vanilla_version_1_21_4.json", 21),
            ("vanilla_version_1_16_5.json", 8),
        ] {
            let body = std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures")
                    .join(fixture),
            )
            .unwrap();
            let artifact = parse_version_detail(&body, "x").unwrap();
            assert_eq!(artifact.java_major, Some(expected), "{fixture}");
        }
    }

    #[tokio::test]
    async fn unknown_versions_are_reported_not_guessed() {
        let err = resolve(&fixtures(), "1.99.9").await.unwrap_err();
        assert_eq!(err.kind(), "version_not_found");
    }

    #[tokio::test]
    async fn resolves_a_real_version_end_to_end() {
        let artifact = resolve(&fixtures(), "1.21.4").await.unwrap();
        assert!(artifact.url.ends_with("server.jar"));
        assert_eq!(artifact.java_major, Some(21));
        assert_eq!(artifact.size, Some(56880250));
    }

    #[tokio::test]
    async fn versions_come_back_in_release_order() {
        let index = index_from_fixture();
        let versions = list_versions(&fixtures(), &index).await.unwrap();

        // Newest first, across both numbering eras and both kinds.
        let dates: Vec<&str> = versions
            .iter()
            .filter_map(|v| v.release_time.as_deref())
            .collect();
        assert!(dates.windows(2).all(|pair| pair[0] >= pair[1]), "{dates:?}");

        // The newest *release* is still the one a picker defaults to.
        let newest_release = versions
            .iter()
            .find(|v| v.kind == crate::mcversion::VersionKind::Release)
            .map(|v| v.id.as_str());
        assert_eq!(newest_release, Some("26.2"));
        assert_eq!(versions.last().map(|v| v.id.as_str()), Some("1.12.2"));
    }

    #[test]
    fn manifest_entries_carry_release_times_in_manifest_order() {
        let body = manifest_body();
        let entries = parse_manifest_entries(&body).unwrap();
        assert_eq!(entries.first().map(|e| e.position), Some(0));
        assert!(entries.iter().all(|e| e.release_time.contains('T')));
        // Manifest order is newest first, so later entries are older.
        let first = &entries[0];
        let last = entries.last().unwrap();
        assert!(first.release_time > last.release_time);
    }

    fn manifest_body() -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/vanilla_version_manifest_v2.json"),
        )
        .unwrap()
    }

    fn index_from_fixture() -> VersionIndex {
        VersionIndex::from_entries(parse_manifest_entries(&manifest_body()).unwrap())
    }
}
