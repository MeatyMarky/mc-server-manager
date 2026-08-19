//! Installing a server into an instance folder.
//!
//! Jar-based server types are a download plus a copy. Forge and NeoForge ship
//! an installer that has to be run, and that is where things go wrong most
//! often, so it runs inside a staging folder: on failure the staging folder is
//! deleted and the instance is left exactly as it was, with the installer's own
//! log kept under `.msm/` for the UI to show.

use std::path::{Path, PathBuf};

use tokio_util::sync::CancellationToken;

use crate::db::models::{Instance, LaunchKind, ServerType};
use crate::db::{now_rfc3339, record_event};
use crate::download;
use crate::error::{AppError, AppResult, IoContext};
use crate::http::Http;
use crate::java;
use crate::paths;
use crate::providers::{self, Artifact, ArtifactKind};
use crate::state::AppState;

/// Phases reported through `task://progress`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Resolve,
    Download,
    Install,
    Finalize,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Resolve => "resolve",
            Phase::Download => "download",
            Phase::Install => "install",
            Phase::Finalize => "finalize",
        }
    }
}

/// Where downloaded artifacts are cached, shared across instances so cloning or
/// reinstalling the same version costs nothing.
pub fn cache_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("cache").join("artifacts")
}

/// Cache file name: the artifact name prefixed with its checksum when one
/// exists, so two builds with the same file name cannot collide.
pub fn cache_path(data_dir: &Path, artifact: &Artifact) -> PathBuf {
    let prefix = artifact
        .sha256
        .as_deref()
        .or(artifact.sha1.as_deref())
        .or(artifact.md5.as_deref())
        .map(|hash| hash[..hash.len().min(12)].to_ascii_lowercase())
        .or_else(|| artifact.build.clone())
        .unwrap_or_else(|| "plain".to_string());
    cache_dir(data_dir).join(format!("{prefix}-{}", artifact.file_name))
}

/// Decides how a freshly installed folder is launched.
///
/// Forge and NeoForge from 1.17 onwards produce `libraries/` plus a platform
/// argument file and no runnable jar; older ones produce a universal jar; both
/// also drop `run.sh`/`run.bat`.
pub fn detect_launch(dir: &Path, server_type: ServerType) -> (LaunchKind, Option<String>) {
    if matches!(server_type, ServerType::Forge | ServerType::NeoForge) {
        let vendor = if server_type == ServerType::Forge {
            "minecraftforge"
        } else {
            "neoforged"
        };
        let artifact = if server_type == ServerType::Forge {
            "forge"
        } else {
            "neoforge"
        };
        let args_name = if cfg!(windows) {
            "win_args.txt"
        } else {
            "unix_args.txt"
        };

        let base: PathBuf = ["libraries", "net", vendor, artifact].iter().collect();
        if let Ok(entries) = std::fs::read_dir(dir.join(&base)) {
            for entry in entries.flatten() {
                let candidate = entry.path().join(args_name);
                if candidate.is_file() {
                    let relative = candidate
                        .strip_prefix(dir)
                        .unwrap_or(&candidate)
                        .to_string_lossy()
                        .to_string();
                    return (LaunchKind::ArgsFile, Some(relative));
                }
            }
        }

        let script = if cfg!(windows) { "run.bat" } else { "run.sh" };
        if dir.join(script).is_file() {
            return (LaunchKind::Script, Some(script.to_string()));
        }

        // Pre-1.17 Forge: a universal jar sits in the server folder.
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let lower = name.to_ascii_lowercase();
                if lower.starts_with("forge-") && lower.ends_with(".jar") && !lower.contains("installer") {
                    return (LaunchKind::Jar, Some(name));
                }
            }
        }
    }

    (LaunchKind::Jar, Some("server.jar".to_string()))
}

/// Everything an install needs to report back.
#[derive(Debug, Clone)]
pub struct InstallOutcome {
    pub launch_kind: LaunchKind,
    pub launch_target: Option<String>,
    pub build: Option<String>,
    pub java_major: i64,
    pub artifact_url: String,
}

/// Full install: resolve, download (resumable, verified), install, record.
pub async fn install<P>(
    state: &AppState,
    http: &Http,
    instance: &Instance,
    mc_version: &str,
    build: Option<&str>,
    cancel: &CancellationToken,
    mut report: P,
) -> AppResult<InstallOutcome>
where
    P: FnMut(Phase, u64, Option<u64>, String) + Send,
{
    let dir = instance.path_buf();
    if !dir.is_dir() {
        return Err(AppError::FolderMissing {
            name: instance.name.clone(),
            path: dir,
        });
    }
    if state.status_of(&instance.uuid).is_live() {
        return Err(AppError::InstanceRunning(instance.name.clone()));
    }

    report(Phase::Resolve, 0, None, format!("Looking up {mc_version}"));
    let artifact = providers::resolve(instance.server_type, http, mc_version, build).await?;
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }

    let cached = cache_path(&state.data_dir, &artifact);
    report(
        Phase::Download,
        0,
        artifact.size,
        format!("Downloading {}", artifact.file_name),
    );
    download::download(http, &artifact, &cached, cancel, |progress| {
        report(
            Phase::Download,
            progress.downloaded,
            progress.total,
            format!("Downloading {}", artifact.file_name),
        );
    })
    .await?;

    record_artifact(state, &artifact, &cached).await?;

    report(Phase::Install, 0, None, "Installing".to_string());
    let outcome = match artifact.kind {
        ArtifactKind::ServerJar => install_jar(&dir, &cached, &artifact, mc_version).await?,
        ArtifactKind::Installer => {
            run_installer(state, instance, &dir, &cached, &artifact, cancel).await?
        }
    };

    report(Phase::Finalize, 1, Some(1), "Finishing up".to_string());
    apply_outcome(state, instance.id, mc_version, &outcome).await?;
    Ok(outcome)
}

/// Writes what the install produced onto the instance row and mirrors it to
/// `.msm/instance.json`. Part of the install, not of the command wrapper: a
/// caller that installs must never be left with a stale launch target.
pub async fn apply_outcome(
    state: &AppState,
    id: i64,
    mc_version: &str,
    outcome: &InstallOutcome,
) -> AppResult<()> {
    let now = now_rfc3339();
    sqlx::query(
        "UPDATE instances SET
            mc_version = ?, loader_version = ?, launch_kind = ?, launch_target = ?,
            java_major = ?, installed_artifact_url = ?, installed_at = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(mc_version)
    .bind(&outcome.build)
    .bind(outcome.launch_kind)
    .bind(&outcome.launch_target)
    .bind(outcome.java_major)
    .bind(&outcome.artifact_url)
    .bind(&now)
    .bind(&now)
    .bind(id)
    .execute(&state.db)
    .await?;

    record_event(
        &state.db,
        id,
        "installed",
        Some(&format!(
            "{mc_version}{}",
            outcome
                .build
                .as_ref()
                .map(|build| format!(" build {build}"))
                .unwrap_or_default()
        )),
    )
    .await?;

    let instance = super::get(&state.db, id).await?;
    super::crud::write_manifest(&instance).await
}

async fn install_jar(
    dir: &Path,
    cached: &Path,
    artifact: &Artifact,
    mc_version: &str,
) -> AppResult<InstallOutcome> {
    let target = dir.join(&artifact.file_name);
    tokio::fs::copy(cached, &target)
        .await
        .ctx("copy server jar", &target)?;

    Ok(InstallOutcome {
        launch_kind: LaunchKind::Jar,
        launch_target: Some(artifact.file_name.clone()),
        build: artifact.build.clone(),
        // The fallback is the Minecraft version, not the build: a Fabric
        // artifact's `build` is its loader version ("0.19.3"), and asking what
        // Java that needs answered 8 — which is how a 26.2 server came to be
        // launched on Java 17.
        java_major: artifact
            .java_major
            .unwrap_or_else(|| java::required_java_for(mc_version)),
        artifact_url: artifact.url.clone(),
    })
}

/// Runs a Forge/NeoForge installer headlessly in a staging folder.
async fn run_installer(
    state: &AppState,
    instance: &Instance,
    dir: &Path,
    installer_jar: &Path,
    artifact: &Artifact,
    cancel: &CancellationToken,
) -> AppResult<InstallOutcome> {
    let installer_name = if instance.server_type == ServerType::Forge {
        "Forge"
    } else {
        "NeoForge"
    };

    let required = java::required_java_for(&instance.mc_version);
    let java_binary = match &instance.java_path {
        Some(pinned) => PathBuf::from(pinned),
        None => java::best_for(&state.db, required)
            .await?
            .map(|runtime| PathBuf::from(runtime.path))
            .ok_or(AppError::JavaNotFound { required })?,
    };

    let staging = paths::msm_dir(dir).join("staging");
    if staging.exists() {
        tokio::fs::remove_dir_all(&staging)
            .await
            .ctx("clear staging folder", &staging)?;
    }
    tokio::fs::create_dir_all(&staging)
        .await
        .ctx("create staging folder", &staging)?;

    let staged_jar = staging.join(&artifact.file_name);
    tokio::fs::copy(installer_jar, &staged_jar)
        .await
        .ctx("stage installer", &staged_jar)?;

    let log_path = paths::msm_dir(dir).join(format!(
        "installer-{}.log",
        now_rfc3339().replace([':', '-'], "").replace('T', "-")
    ));

    let output = {
        let mut command = tokio::process::Command::new(&java_binary);
        command
            .current_dir(&staging)
            .arg("-jar")
            .arg(&staged_jar)
            .arg("--installServer")
            .arg(&staging);
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                let _ = tokio::fs::remove_dir_all(&staging).await;
                return Err(AppError::Cancelled);
            }
            result = command.output() => result,
        }
    }
    .map_err(|e| AppError::io("run the installer", &java_binary, e))?;

    // The installer's own words are kept whatever happens: they are the only
    // useful diagnostic when it fails.
    let transcript = format!(
        "command: {} -jar {} --installServer {}\nexit: {}\n\n--- stdout ---\n{}\n--- stderr ---\n{}\n",
        java_binary.display(),
        staged_jar.display(),
        staging.display(),
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    tokio::fs::write(&log_path, transcript.as_bytes())
        .await
        .ctx("write installer log", &log_path)?;

    if !output.status.success() {
        // Leave nothing half-written behind, so a retry starts clean.
        let _ = tokio::fs::remove_dir_all(&staging).await;
        record_event(
            &state.db,
            instance.id,
            "error",
            Some(&format!("{installer_name} installer failed")),
        )
        .await?;

        return Err(AppError::InstallerFailed {
            installer: installer_name,
            exit_code: output.status.code().unwrap_or(-1),
            log_path: log_path.to_string_lossy().to_string(),
            log_tail: tail(&transcript, 40),
        });
    }

    // Success: move the produced files into the instance, minus the installer.
    move_installed_files(&staging, dir, &artifact.file_name).await?;
    let _ = tokio::fs::remove_dir_all(&staging).await;

    let (launch_kind, launch_target) = detect_launch(dir, instance.server_type);
    Ok(InstallOutcome {
        launch_kind,
        launch_target,
        build: artifact.build.clone(),
        java_major: required,
        artifact_url: artifact.url.clone(),
    })
}

/// Last `lines` lines, which is what the UI shows inline.
pub fn tail(text: &str, lines: usize) -> String {
    let collected: Vec<&str> = text.lines().collect();
    let start = collected.len().saturating_sub(lines);
    collected[start..].join("\n")
}

async fn move_installed_files(staging: &Path, dir: &Path, installer_name: &str) -> AppResult<()> {
    let mut entries = tokio::fs::read_dir(staging)
        .await
        .ctx("read staging folder", staging)?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .ctx("read staging folder", staging)?
    {
        let name = entry.file_name();
        let name_str = name.to_string_lossy().to_string();
        if name_str == installer_name || name_str.ends_with(".jar.log") {
            continue;
        }

        let destination = dir.join(&name);
        if destination.exists() {
            if destination.is_dir() {
                merge_dir(&entry.path(), &destination).await?;
                continue;
            }
            tokio::fs::remove_file(&destination)
                .await
                .ctx("replace file", &destination)?;
        }

        // A rename across the same volume is cheap; staging lives inside the
        // instance folder precisely so this stays a rename.
        tokio::fs::rename(entry.path(), &destination)
            .await
            .ctx("move installed file", &destination)?;
    }
    Ok(())
}

async fn merge_dir(from: &Path, to: &Path) -> AppResult<()> {
    let from = from.to_path_buf();
    let to = to.to_path_buf();
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        for entry in walkdir::WalkDir::new(&from).min_depth(1).into_iter().flatten() {
            let relative = entry.path().strip_prefix(&from).unwrap_or(entry.path());
            let destination = to.join(relative);
            if entry.file_type().is_dir() {
                std::fs::create_dir_all(&destination).ctx("create folder", &destination)?;
            } else if entry.file_type().is_file() {
                if let Some(parent) = destination.parent() {
                    std::fs::create_dir_all(parent).ctx("create folder", parent)?;
                }
                std::fs::copy(entry.path(), &destination).ctx("copy file", entry.path())?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| AppError::internal("merging the installed files", e))?
}

async fn record_artifact(state: &AppState, artifact: &Artifact, path: &Path) -> AppResult<()> {
    let size = tokio::fs::metadata(path).await.map(|m| m.len()).unwrap_or(0);
    sqlx::query(
        "INSERT INTO artifact_cache (url, sha1, sha256, path, size_bytes, fetched_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(url) DO UPDATE SET
            sha1 = excluded.sha1, sha256 = excluded.sha256, path = excluded.path,
            size_bytes = excluded.size_bytes, fetched_at = excluded.fetched_at",
    )
    .bind(&artifact.url)
    .bind(&artifact.sha1)
    .bind(&artifact.sha256)
    .bind(path.to_string_lossy().to_string())
    .bind(size as i64)
    .bind(now_rfc3339())
    .execute(&state.db)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ArtifactKind;

    fn artifact(name: &str) -> Artifact {
        Artifact {
            url: format!("https://example.invalid/{name}"),
            file_name: name.to_string(),
            kind: ArtifactKind::ServerJar,
            sha1: None,
            sha256: None,
            sha512: None,
            md5: None,
            size: None,
            build: None,
            java_major: None,
        }
    }

    #[test]
    fn cache_paths_are_namespaced_by_checksum() {
        let data = PathBuf::from("data");
        let mut a = artifact("paper-1.21.4-232.jar");
        a.sha256 = Some("5EE4F542F628A14C".into());
        let path = cache_path(&data, &a);
        assert!(path.starts_with(cache_dir(&data)));
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            "5ee4f542f628-paper-1.21.4-232.jar"
        );
    }

    #[test]
    fn cache_paths_fall_back_to_the_build_then_to_plain() {
        let data = PathBuf::from("data");
        let mut a = artifact("server.jar");
        a.build = Some("54.1.6".into());
        assert!(cache_path(&data, &a)
            .to_string_lossy()
            .ends_with("54.1.6-server.jar"));

        let plain = cache_path(&data, &artifact("server.jar"));
        assert!(plain.to_string_lossy().ends_with("plain-server.jar"));
    }

    #[test]
    fn jar_servers_launch_from_server_jar() {
        let dir = tempfile::tempdir().unwrap();
        let (kind, target) = detect_launch(dir.path(), ServerType::Paper);
        assert_eq!(kind, LaunchKind::Jar);
        assert_eq!(target.as_deref(), Some("server.jar"));
    }

    #[test]
    fn modern_neoforge_launches_from_its_args_file() {
        let dir = tempfile::tempdir().unwrap();
        let version_dir: PathBuf = ["libraries", "net", "neoforged", "neoforge", "26.2.0.62"]
            .iter()
            .collect();
        std::fs::create_dir_all(dir.path().join(&version_dir)).unwrap();
        for name in ["win_args.txt", "unix_args.txt"] {
            std::fs::write(dir.path().join(&version_dir).join(name), b"@libraries").unwrap();
        }

        let (kind, target) = detect_launch(dir.path(), ServerType::NeoForge);
        assert_eq!(kind, LaunchKind::ArgsFile);
        let target = target.unwrap();
        // The platform picks the file; both are on disk.
        if cfg!(windows) {
            assert!(target.ends_with("win_args.txt"), "{target}");
        } else {
            assert!(target.ends_with("unix_args.txt"), "{target}");
        }
        assert!(target.starts_with("libraries"));
    }

    #[test]
    fn forge_falls_back_to_the_run_script_then_the_universal_jar() {
        let script_dir = tempfile::tempdir().unwrap();
        let script = if cfg!(windows) { "run.bat" } else { "run.sh" };
        std::fs::write(script_dir.path().join(script), b"java @args").unwrap();
        let (kind, target) = detect_launch(script_dir.path(), ServerType::Forge);
        assert_eq!(kind, LaunchKind::Script);
        assert_eq!(target.as_deref(), Some(script));

        let jar_dir = tempfile::tempdir().unwrap();
        std::fs::write(jar_dir.path().join("forge-1.12.2-14.23.5.2859.jar"), b"jar").unwrap();
        std::fs::write(jar_dir.path().join("forge-1.12.2-installer.jar"), b"jar").unwrap();
        let (kind, target) = detect_launch(jar_dir.path(), ServerType::Forge);
        assert_eq!(kind, LaunchKind::Jar);
        assert_eq!(target.as_deref(), Some("forge-1.12.2-14.23.5.2859.jar"));
    }

    #[test]
    fn tail_keeps_the_last_lines() {
        let text = (1..=100).map(|n| n.to_string()).collect::<Vec<_>>().join("\n");
        let tail = tail(&text, 3);
        assert_eq!(tail, "98\n99\n100");
        assert_eq!(super::tail("one line", 10), "one line");
    }

    #[tokio::test]
    async fn installed_files_move_out_of_staging_without_the_installer() {
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("staging");
        std::fs::create_dir_all(staging.join("libraries").join("net")).unwrap();
        std::fs::write(staging.join("libraries").join("net").join("a.jar"), b"x").unwrap();
        std::fs::write(staging.join("run.sh"), b"#!/bin/sh").unwrap();
        std::fs::write(staging.join("forge-installer.jar"), b"installer").unwrap();

        let target = dir.path().join("instance");
        std::fs::create_dir_all(&target).unwrap();
        move_installed_files(&staging, &target, "forge-installer.jar")
            .await
            .unwrap();

        assert!(target.join("libraries").join("net").join("a.jar").is_file());
        assert!(target.join("run.sh").is_file());
        assert!(!target.join("forge-installer.jar").exists());
    }
}
