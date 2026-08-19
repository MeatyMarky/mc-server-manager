//! Resolving a JDK to download from the Adoptium API.
//!
//! Only the `package` of a release is used, never the `installer`: an `.msi` or
//! `.pkg` would install into the system, and a managed runtime has to live in
//! this app's own folder and disappear with it. The archive carries a SHA-256
//! that the download engine verifies before anything is unpacked.

use serde::Deserialize;

use crate::error::{AppError, AppResult};
use crate::http::Fetch;
use crate::providers::{Artifact, ArtifactKind};

pub const API_ROOT: &str = "https://api.adoptium.net/v3";

/// One resolved JDK: what it is, and what downloading it costs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Feature version: 8, 17, 21, 25.
    pub feature_version: i64,
    /// `jdk-25.0.4+7`, which is what the folder is named after.
    pub release_name: String,
    /// `25.0.4+7-LTS`, for display.
    pub openjdk_version: String,
    pub url: String,
    pub file_name: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub os: String,
    pub arch: String,
}

impl Candidate {
    /// The download engine speaks in artifacts, so a candidate becomes one.
    pub fn artifact(&self) -> Artifact {
        Artifact {
            url: self.url.clone(),
            file_name: self.file_name.clone(),
            kind: ArtifactKind::ServerJar,
            sha1: None,
            sha256: Some(self.sha256.clone()),
            sha512: None,
            md5: None,
            size: Some(self.size_bytes),
            build: Some(self.release_name.clone()),
            java_major: Some(self.feature_version),
        }
    }

    /// Folder this runtime installs into, under the managed-runtimes directory.
    pub fn install_dir_name(&self) -> String {
        format!("temurin-{}", self.feature_version)
    }
}

/// The operating system name Adoptium uses for the machine this is running on.
pub fn current_os() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "mac"
    } else {
        "linux"
    }
}

/// The architecture name Adoptium uses for this machine.
///
/// Only 64-bit targets are named: a 32-bit JVM is never a candidate for a
/// server, so asking for one would only produce a runtime this app refuses.
pub fn current_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "aarch64",
        "arm" => "arm",
        "powerpc64" => "ppc64le",
        "s390x" => "s390x",
        other => other,
    }
}

pub fn latest_url(feature_version: i64, os: &str, arch: &str) -> String {
    format!(
        "{API_ROOT}/assets/latest/{feature_version}/hotspot\
         ?os={os}&architecture={arch}&image_type=jdk&vendor=eclipse"
    )
}

/// Resolves the newest Temurin JDK for a feature version on this platform.
pub async fn resolve<F: Fetch>(
    fetch: &F,
    feature_version: i64,
    os: &str,
    arch: &str,
) -> AppResult<Candidate> {
    let url = latest_url(feature_version, os, arch);
    let releases: Vec<Release> = fetch.get_json(&url).await?;
    parse(releases, feature_version, os, arch)
}

/// The shape of the API response, kept private so no Adoptium type escapes.
#[derive(Debug, Deserialize)]
struct Release {
    release_name: String,
    version: Version,
    binary: Binary,
}

#[derive(Debug, Deserialize)]
struct Version {
    major: i64,
    openjdk_version: String,
}

#[derive(Debug, Deserialize)]
struct Binary {
    os: String,
    architecture: String,
    image_type: String,
    /// Absent for releases that ship only an installer.
    package: Option<Package>,
}

#[derive(Debug, Deserialize)]
struct Package {
    name: String,
    link: String,
    checksum: Option<String>,
    size: Option<u64>,
}

fn parse(
    releases: Vec<Release>,
    feature_version: i64,
    os: &str,
    arch: &str,
) -> AppResult<Candidate> {
    let mut best: Option<Candidate> = None;

    for release in releases {
        if release.binary.image_type != "jdk" {
            continue;
        }
        let Some(package) = release.binary.package else {
            continue;
        };
        // An archive without a checksum cannot be verified, and an unverified
        // JDK is not something to unpack and then execute.
        let Some(checksum) = package.checksum.filter(|sum| sum.len() == 64) else {
            continue;
        };

        let candidate = Candidate {
            feature_version: release.version.major,
            release_name: release.release_name,
            openjdk_version: release.version.openjdk_version,
            url: package.link,
            file_name: package.name,
            sha256: checksum.to_ascii_lowercase(),
            size_bytes: package.size.unwrap_or(0),
            os: release.binary.os,
            arch: release.binary.architecture,
        };

        // Newest wins when the API returns more than one.
        let newer = match best.as_ref() {
            Some(current) => candidate.release_name > current.release_name,
            None => true,
        };
        if newer {
            best = Some(candidate);
        }
    }

    best.ok_or_else(|| AppError::VersionNotFound {
        kind: "Temurin JDK",
        version: format!("{feature_version} for {os}/{arch}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::FixtureFetch;

    #[tokio::test]
    async fn a_windows_jdk_25_resolves_to_a_verifiable_archive() {
        let url = latest_url(25, "windows", "x64");
        let fetch = FixtureFetch::new().route(&url, "adoptium_latest_25_windows_x64.json");

        let candidate = resolve(&fetch, 25, "windows", "x64").await.unwrap();

        assert_eq!(candidate.feature_version, 25);
        assert_eq!(candidate.os, "windows");
        assert_eq!(candidate.arch, "x64");
        assert!(candidate.release_name.starts_with("jdk-25"));
        assert!(candidate.openjdk_version.starts_with("25."));

        // The archive, never the installer: an .msi would install system-wide.
        assert!(candidate.file_name.ends_with(".zip"), "{}", candidate.file_name);
        assert!(!candidate.url.contains(".msi"), "{}", candidate.url);

        // Verifiable and sized, which is what the confirmation dialog shows.
        assert_eq!(candidate.sha256.len(), 64);
        assert_eq!(candidate.sha256, candidate.sha256.to_ascii_lowercase());
        assert!(candidate.size_bytes > 50_000_000, "{}", candidate.size_bytes);

        // And it becomes an artifact the existing download engine can verify.
        let artifact = candidate.artifact();
        assert_eq!(artifact.sha256.as_deref(), Some(candidate.sha256.as_str()));
        assert_eq!(artifact.size, Some(candidate.size_bytes));
        assert_eq!(
            crate::download::expected_checksum(&artifact).unwrap().0,
            crate::download::Algorithm::Sha256
        );
    }

    #[tokio::test]
    async fn linux_and_older_lines_resolve_the_same_way() {
        for (fixture_name, feature, os, arch, extension) in [
            ("adoptium_latest_21_linux_x64.json", 21, "linux", "x64", ".tar.gz"),
            ("adoptium_latest_17_windows_x64.json", 17, "windows", "x64", ".zip"),
            ("adoptium_latest_8_linux_aarch64.json", 8, "linux", "aarch64", ".tar.gz"),
        ] {
            let url = latest_url(feature, os, arch);
            let fetch = FixtureFetch::new().route(&url, fixture_name);
            let candidate = resolve(&fetch, feature, os, arch).await.unwrap();

            assert_eq!(candidate.feature_version, feature, "{fixture_name}");
            assert_eq!(candidate.os, os, "{fixture_name}");
            assert_eq!(candidate.arch, arch, "{fixture_name}");
            assert!(
                candidate.file_name.ends_with(extension),
                "{fixture_name}: {}",
                candidate.file_name
            );
            assert_eq!(candidate.sha256.len(), 64, "{fixture_name}");
            assert_eq!(candidate.install_dir_name(), format!("temurin-{feature}"));
        }
    }

    #[test]
    fn the_query_names_hotspot_jdk_and_eclipse_explicitly() {
        let url = latest_url(21, "linux", "x64");
        assert!(url.starts_with(API_ROOT));
        assert!(url.contains("/assets/latest/21/hotspot"));
        assert!(url.contains("os=linux"));
        assert!(url.contains("architecture=x64"));
        assert!(url.contains("image_type=jdk"), "a JRE cannot run an installer");
        assert!(url.contains("vendor=eclipse"));
    }

    #[test]
    fn this_platform_maps_onto_names_the_api_knows() {
        assert!(["windows", "linux", "mac"].contains(&current_os()));
        assert!(["x64", "aarch64", "arm", "ppc64le", "s390x"].contains(&current_arch()));
    }

    #[test]
    fn a_release_without_a_usable_archive_is_not_offered() {
        // Installer-only, no package: nothing to unpack into a managed folder.
        let installer_only = r#"[{
            "release_name": "jdk-21.0.1+12",
            "version": { "major": 21, "openjdk_version": "21.0.1+12" },
            "binary": { "os": "windows", "architecture": "x64", "image_type": "jdk" }
        }]"#;
        let releases: Vec<Release> = serde_json::from_str(installer_only).unwrap();
        assert!(parse(releases, 21, "windows", "x64").is_err());

        // A package with no checksum cannot be verified, so it is refused
        // rather than downloaded and trusted.
        let unverifiable = r#"[{
            "release_name": "jdk-21.0.1+12",
            "version": { "major": 21, "openjdk_version": "21.0.1+12" },
            "binary": { "os": "windows", "architecture": "x64", "image_type": "jdk",
                "package": { "name": "x.zip", "link": "https://example/x.zip", "size": 1 } }
        }]"#;
        let releases: Vec<Release> = serde_json::from_str(unverifiable).unwrap();
        let err = parse(releases, 21, "windows", "x64").unwrap_err();
        assert_eq!(err.kind(), "version_not_found");
    }

    #[test]
    fn the_newest_release_wins_when_several_come_back() {
        let two = r#"[
            { "release_name": "jdk-21.0.1+12",
              "version": { "major": 21, "openjdk_version": "21.0.1+12" },
              "binary": { "os": "linux", "architecture": "x64", "image_type": "jdk",
                "package": { "name": "old.tar.gz", "link": "https://example/old",
                  "checksum": "1111111111111111111111111111111111111111111111111111111111111111",
                  "size": 10 } } },
            { "release_name": "jdk-21.0.9+11",
              "version": { "major": 21, "openjdk_version": "21.0.9+11" },
              "binary": { "os": "linux", "architecture": "x64", "image_type": "jdk",
                "package": { "name": "new.tar.gz", "link": "https://example/new",
                  "checksum": "2222222222222222222222222222222222222222222222222222222222222222",
                  "size": 20 } } }
        ]"#;
        let releases: Vec<Release> = serde_json::from_str(two).unwrap();
        let candidate = parse(releases, 21, "linux", "x64").unwrap();
        assert_eq!(candidate.release_name, "jdk-21.0.9+11");
    }
}
