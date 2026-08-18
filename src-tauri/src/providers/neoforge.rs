//! NeoForge, via its Maven repository.
//!
//! NeoForge encodes the Minecraft version in its own version number, and the
//! encoding changed with the calendar era:
//!   * three components — `21.1.65` is Minecraft 1.21.1, `21.0.167` is 1.21
//!   * four components  — `26.2.0.62` is Minecraft 26.2, `26.1.2.95` is 26.1.2
//!
//! Getting that mapping wrong picks the wrong loader for a version, so it is a
//! pure function with tests for both eras.

use crate::error::{AppError, AppResult};
use crate::http::Fetch;
use crate::mcversion;

use super::{parse_maven_versions, Artifact, ArtifactKind, BuildEntry, VersionEntry};

pub const MAVEN_ROOT: &str = "https://maven.neoforged.net/releases/net/neoforged/neoforge";

pub fn metadata_url() -> String {
    format!("{MAVEN_ROOT}/maven-metadata.xml")
}

pub fn installer_url(neoforge_version: &str) -> String {
    format!("{MAVEN_ROOT}/{neoforge_version}/neoforge-{neoforge_version}-installer.jar")
}

/// The Minecraft version a NeoForge version targets.
pub fn mc_version_for(neoforge_version: &str) -> Option<String> {
    let numeric = neoforge_version
        .split('-')
        .next()
        .unwrap_or(neoforge_version);
    let parts: Vec<u32> = numeric
        .split('.')
        .map(|p| p.parse::<u32>().ok())
        .collect::<Option<Vec<_>>>()?;

    match parts.as_slice() {
        // Calendar era: <year>.<release>.<patch>.<build>
        [year, release, patch, _build] if *year >= 20 => Some(if *patch == 0 {
            format!("{year}.{release}")
        } else {
            format!("{year}.{release}.{patch}")
        }),
        // Classic era: <minor>.<patch>.<build> for Minecraft 1.<minor>.<patch>
        [minor, patch, _build] => Some(if *patch == 0 {
            format!("1.{minor}")
        } else {
            format!("1.{minor}.{patch}")
        }),
        _ => None,
    }
}

pub fn is_stable(neoforge_version: &str) -> bool {
    !neoforge_version.contains("-beta") && !neoforge_version.contains("-alpha")
}

/// Distinct Minecraft versions covered by the published loaders, newest first.
pub fn versions_from_metadata(xml: &str) -> Vec<VersionEntry> {
    let mut seen: Vec<String> = Vec::new();
    for version in parse_maven_versions(xml) {
        if let Some(mc) = mc_version_for(&version) {
            if !seen.contains(&mc) {
                seen.push(mc);
            }
        }
    }
    mcversion::sort_newest_first(&mut seen);
    seen.into_iter()
        .map(|id| VersionEntry { id, stable: true })
        .collect()
}

/// Loader builds for one Minecraft version, newest first.
pub fn builds_from_metadata(xml: &str, mc_version: &str) -> Vec<BuildEntry> {
    let mut builds: Vec<BuildEntry> = parse_maven_versions(xml)
        .into_iter()
        .filter(|v| mc_version_for(v).as_deref() == Some(mc_version))
        .map(|v| BuildEntry {
            stable: is_stable(&v),
            label: (!is_stable(&v)).then(|| "beta".to_string()),
            id: v,
        })
        .collect();
    builds.reverse();
    builds
}

pub async fn list_versions<F: Fetch>(fetch: &F) -> AppResult<Vec<VersionEntry>> {
    Ok(versions_from_metadata(&fetch.get_text(&metadata_url()).await?))
}

pub async fn list_builds<F: Fetch>(fetch: &F, mc_version: &str) -> AppResult<Vec<BuildEntry>> {
    Ok(builds_from_metadata(
        &fetch.get_text(&metadata_url()).await?,
        mc_version,
    ))
}

pub async fn resolve<F: Fetch>(
    fetch: &F,
    mc_version: &str,
    build: Option<&str>,
) -> AppResult<Artifact> {
    let xml = fetch.get_text(&metadata_url()).await?;
    let builds = builds_from_metadata(&xml, mc_version);

    let chosen = match build {
        Some(wanted) => builds.iter().find(|b| b.id == wanted).cloned(),
        None => builds
            .iter()
            .find(|b| b.stable)
            .or_else(|| builds.first())
            .cloned(),
    }
    .ok_or_else(|| AppError::VersionNotFound {
        kind: "NeoForge",
        version: match build {
            Some(b) => b.to_string(),
            None => mc_version.to_string(),
        },
    })?;

    Ok(Artifact {
        url: installer_url(&chosen.id),
        file_name: format!("neoforge-{}-installer.jar", chosen.id),
        kind: ArtifactKind::Installer,
        sha1: None,
        sha256: None,
        md5: None,
        size: None,
        build: Some(chosen.id),
        java_major: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::FixtureFetch;

    fn fixtures() -> FixtureFetch {
        FixtureFetch::new().route(&metadata_url(), "neoforge_maven_metadata.xml")
    }

    #[test]
    fn maps_classic_era_versions() {
        assert_eq!(mc_version_for("21.1.65").as_deref(), Some("1.21.1"));
        assert_eq!(mc_version_for("21.0.167").as_deref(), Some("1.21"));
        assert_eq!(mc_version_for("20.4.237").as_deref(), Some("1.20.4"));
        assert_eq!(mc_version_for("20.4.10-beta").as_deref(), Some("1.20.4"));
    }

    #[test]
    fn maps_calendar_era_versions() {
        assert_eq!(mc_version_for("26.2.0.62").as_deref(), Some("26.2"));
        assert_eq!(mc_version_for("26.1.2.95").as_deref(), Some("26.1.2"));
        assert_eq!(mc_version_for("26.2.0.1-beta").as_deref(), Some("26.2"));
    }

    #[test]
    fn rejects_nonsense_versions() {
        assert_eq!(mc_version_for("nonsense"), None);
        assert_eq!(mc_version_for("1"), None);
    }

    #[test]
    fn beta_builds_are_marked_unstable() {
        assert!(is_stable("26.2.0.62"));
        assert!(!is_stable("26.2.0.1-beta"));
    }

    #[tokio::test]
    async fn versions_cover_both_eras_newest_first() {
        let versions = list_versions(&fixtures()).await.unwrap();
        let ids: Vec<&str> = versions.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids.first(), Some(&"26.2"));
        assert!(ids.contains(&"1.20.4") || ids.contains(&"1.21.1"));
    }

    #[tokio::test]
    async fn resolve_prefers_a_stable_build_and_builds_the_installer_url() {
        let artifact = resolve(&fixtures(), "26.2", None).await.unwrap();
        assert_eq!(artifact.kind, ArtifactKind::Installer);
        let build = artifact.build.clone().unwrap();
        assert!(is_stable(&build), "picked {build}");
        assert_eq!(
            artifact.url,
            format!("{MAVEN_ROOT}/{build}/neoforge-{build}-installer.jar")
        );
    }

    #[tokio::test]
    async fn an_unknown_build_is_rejected() {
        let err = resolve(&fixtures(), "26.2", Some("99.9.9.9")).await.unwrap_err();
        assert_eq!(err.kind(), "version_not_found");
    }
}
