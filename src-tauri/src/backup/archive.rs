//! Writing and reading backup archives.
//!
//! Two formats: `zip`, which anything can open, and `tar.zst`, which is faster
//! and smaller and is the default. Both stream, report progress per entry, check
//! for cancellation between entries, and only take their final name once the
//! archive is complete — a cancelled backup never leaves a file that looks like
//! a finished one.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

use crate::error::{AppError, AppResult, IoContext};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type, TS)]
#[sqlx(rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub enum Format {
    /// `tar.zst`: the default, because it is faster to write and smaller.
    TarZst,
    /// `zip`: portable, openable by anything.
    Zip,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::TarZst => "tar.zst",
            Format::Zip => "zip",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Format::TarZst => "tar_zst",
            Format::Zip => "zip",
        }
    }

    /// Guesses from a file name, for archives created before a setting changed.
    pub fn from_path(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
        if name.ends_with(".tar.zst") {
            Some(Format::TarZst)
        } else if name.ends_with(".zip") {
            Some(Format::Zip)
        } else {
            None
        }
    }

    /// Clamps a level to what the format can use.
    pub fn clamp_level(self, level: i32) -> i32 {
        match self {
            // zstd accepts 1..=22; 3 is its own default and a good trade.
            Format::TarZst => level.clamp(1, 22),
            // Deflate accepts 0..=9.
            Format::Zip => level.clamp(0, 9),
        }
    }

    pub fn default_level(self) -> i32 {
        match self {
            Format::TarZst => 3,
            Format::Zip => 6,
        }
    }
}

/// What goes into the archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type, TS)]
#[sqlx(rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub enum Scope {
    /// Everything except the always-excluded noise.
    Full,
    /// Only the world folders.
    Worlds,
}

/// Paths never worth archiving: regenerated, machine-specific, or huge and
/// meaningless. `.msm/staging` in particular can hold a half-unpacked modpack.
pub const ALWAYS_EXCLUDED: &[&str] = &[
    "logs",
    "crash-reports",
    "cache",
    "libraries",
    "versions",
    "debug",
];

/// Decides whether one entry, relative to the instance folder, is archived.
///
/// `worlds` names the world folders, which is how `Scope::Worlds` knows what to
/// keep without guessing from names.
pub fn include_entry(relative: &Path, scope: Scope, worlds: &[String], extra: &[String]) -> bool {
    let components: Vec<String> = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect();
    let Some(first) = components.first() else {
        return false;
    };

    // Our own metadata: the console captures and any staging folder are noise,
    // but instance.json is worth keeping so a restored folder can be imported.
    // A worlds-only backup takes none of it.
    if first == crate::paths::MSM_DIR {
        return scope == Scope::Full
            && components
                .get(1)
                .map(|second| second == "instance.json")
                .unwrap_or(false);
    }
    if ALWAYS_EXCLUDED.contains(&first.as_str()) {
        return false;
    }
    if extra.iter().any(|pattern| matches_pattern(&components, pattern)) {
        return false;
    }

    let last = components.last().map(String::as_str).unwrap_or_default();
    if last == "session.lock" {
        return false;
    }
    let lower = last.to_ascii_lowercase();
    if lower.ends_with(".log") || lower.ends_with(".log.gz") {
        return false;
    }

    match scope {
        Scope::Full => true,
        Scope::Worlds => worlds.iter().any(|world| world == first),
    }
}

/// A user-supplied exclusion: a folder name, a path prefix, or `*.ext`.
fn matches_pattern(components: &[String], pattern: &str) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return false;
    }
    if let Some(extension) = pattern.strip_prefix("*.") {
        return components
            .last()
            .map(|name| name.to_ascii_lowercase().ends_with(&format!(".{}", extension.to_ascii_lowercase())))
            .unwrap_or(false);
    }

    let wanted: Vec<&str> = pattern
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect();
    components
        .iter()
        .zip(wanted.iter())
        .filter(|(component, part)| component.as_str() == **part)
        .count()
        == wanted.len()
}

/// Files an archive would contain, and how big they are uncompressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct Estimate {
    #[ts(type = "number")]
    pub files: u64,
    /// Uncompressed bytes. Compression only ever makes the archive smaller, so
    /// this is a safe upper bound for the free-space check.
    #[ts(type = "number")]
    pub bytes: u64,
}

/// Walks the instance and measures what would be archived. Blocking.
pub fn estimate(
    instance_dir: &Path,
    scope: Scope,
    worlds: &[String],
    extra: &[String],
) -> AppResult<Estimate> {
    let mut files = 0u64;
    let mut bytes = 0u64;

    for entry in walkdir::WalkDir::new(instance_dir).min_depth(1).into_iter().flatten() {
        let Ok(relative) = entry.path().strip_prefix(instance_dir) else {
            continue;
        };
        if !include_entry(relative, scope, worlds, extra) {
            continue;
        }
        if entry.file_type().is_file() {
            files += 1;
            bytes += entry.metadata().map(|meta| meta.len()).unwrap_or(0);
        }
    }

    Ok(Estimate { files, bytes })
}

#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_read: u64,
}

/// What goes into an archive and how it is written.
///
/// One struct rather than eight positional arguments: the selection (scope,
/// worlds, extra excludes) is the same set the estimate walks, so the two stay
/// in step by construction.
#[derive(Debug, Clone)]
pub struct Spec {
    pub instance_dir: PathBuf,
    pub target: PathBuf,
    pub format: Format,
    pub level: i32,
    pub scope: Scope,
    /// The world folders in the instance, which is what a worlds-only scope keeps.
    pub worlds: Vec<String>,
    /// Extra paths or `*.ext` patterns the user asked to leave out.
    pub extra: Vec<String>,
}

/// Writes the archive. Returns its size on disk.
pub fn write<P>(spec: &Spec, cancel: &CancellationToken, mut report: P) -> AppResult<u64>
where
    P: FnMut(Progress),
{
    let Spec {
        instance_dir,
        target,
        format,
        level,
        scope,
        worlds,
        extra,
    } = spec;
    let (instance_dir, target) = (instance_dir.as_path(), target.as_path());
    let (format, level, scope) = (*format, *level, *scope);

    let files: Vec<PathBuf> = walkdir::WalkDir::new(instance_dir)
        .min_depth(1)
        .into_iter()
        .flatten()
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| {
            entry
                .path()
                .strip_prefix(instance_dir)
                .map(|relative| include_entry(relative, scope, worlds, extra))
                .unwrap_or(false)
        })
        .map(|entry| entry.path().to_path_buf())
        .collect();

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).ctx("create the backup folder", parent)?;
    }
    // Written under a temporary name so a cancelled backup leaves nothing that
    // looks finished.
    let temp = target.with_extension("part");

    let result = match format {
        Format::Zip => write_zip(instance_dir, &files, &temp, level, cancel, &mut report),
        Format::TarZst => write_tar_zst(instance_dir, &files, &temp, level, cancel, &mut report),
    };

    if let Err(err) = result {
        let _ = std::fs::remove_file(&temp);
        return Err(err);
    }

    std::fs::rename(&temp, target).ctx("finish the archive", target)?;
    Ok(std::fs::metadata(target).map(|meta| meta.len()).unwrap_or(0))
}

fn write_zip<P>(
    instance_dir: &Path,
    files: &[PathBuf],
    temp: &Path,
    level: i32,
    cancel: &CancellationToken,
    report: &mut P,
) -> AppResult<()>
where
    P: FnMut(Progress),
{
    let file = std::fs::File::create(temp).ctx("create the archive", temp)?;
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .compression_level(Some(level as i64));

    let total = files.len() as u64;
    let mut buffer = vec![0u8; 128 * 1024];
    let mut bytes_read = 0u64;

    for (position, path) in files.iter().enumerate() {
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let relative = path.strip_prefix(instance_dir).unwrap_or(path);
        zip.start_file(to_archive_path(relative), options)
            .map_err(|e| AppError::internal("writing the archive", e))?;

        let mut source = std::fs::File::open(path).ctx("read file", path)?;
        loop {
            let read = source.read(&mut buffer).ctx("read file", path)?;
            if read == 0 {
                break;
            }
            zip.write_all(&buffer[..read])
                .map_err(|e| AppError::internal("writing the archive", e))?;
            bytes_read += read as u64;
        }

        if position % 32 == 0 || position as u64 + 1 == total {
            report(Progress {
                files_done: position as u64 + 1,
                files_total: total,
                bytes_read,
            });
        }
    }

    zip.finish()
        .map_err(|e| AppError::internal("writing the archive", e))?;
    Ok(())
}

fn write_tar_zst<P>(
    instance_dir: &Path,
    files: &[PathBuf],
    temp: &Path,
    level: i32,
    cancel: &CancellationToken,
    report: &mut P,
) -> AppResult<()>
where
    P: FnMut(Progress),
{
    let file = std::fs::File::create(temp).ctx("create the archive", temp)?;
    let encoder = zstd::stream::Encoder::new(file, level)
        .map_err(|e| AppError::internal("writing the archive", e))?;
    let mut tar = tar::Builder::new(encoder);

    let total = files.len() as u64;
    let mut bytes_read = 0u64;

    for (position, path) in files.iter().enumerate() {
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let relative = path.strip_prefix(instance_dir).unwrap_or(path);
        bytes_read += std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);

        tar.append_path_with_name(path, to_archive_path(relative))
            .map_err(|e| AppError::internal("writing the archive", format!("{}: {e}", relative.display())))?;

        if position % 32 == 0 || position as u64 + 1 == total {
            report(Progress {
                files_done: position as u64 + 1,
                files_total: total,
                bytes_read,
            });
        }
    }

    let encoder = tar
        .into_inner()
        .map_err(|e| AppError::internal("writing the archive", e))?;
    encoder
        .finish()
        .map_err(|e| AppError::internal("writing the archive", e))?;
    Ok(())
}

/// Archive entry paths always use forward slashes, on every platform.
fn to_archive_path(relative: &Path) -> String {
    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

/// One entry of an archive, for the restore preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct ArchiveEntry {
    pub path: String,
    #[ts(type = "number")]
    pub size: u64,
}

/// Lists what an archive holds, without extracting it.
pub fn list(archive: &Path) -> AppResult<Vec<ArchiveEntry>> {
    let format = Format::from_path(archive).ok_or_else(|| {
        AppError::Other(format!(
            "{} is not a backup archive this build understands",
            archive.display()
        ))
    })?;

    let file = std::fs::File::open(archive).ctx("open the archive", archive)?;
    let mut entries = Vec::new();

    match format {
        Format::Zip => {
            let mut zip = zip::ZipArchive::new(file)
                .map_err(|e| AppError::archive(archive.display(), e))?;
            for index in 0..zip.len() {
                let entry = zip
                    .by_index(index)
                    .map_err(|e| AppError::archive(archive.display(), e))?;
                if entry.is_file() {
                    entries.push(ArchiveEntry {
                        path: entry.name().to_string(),
                        size: entry.size(),
                    });
                }
            }
        }
        Format::TarZst => {
            let decoder = zstd::stream::Decoder::new(file)
                .map_err(|e| AppError::archive(archive.display(), e))?;
            let mut tar = tar::Archive::new(decoder);
            for entry in tar
                .entries()
                .map_err(|e| AppError::archive(archive.display(), e))?
            {
                let entry =
                    entry.map_err(|e| AppError::archive(archive.display(), e))?;
                if entry.header().entry_type().is_file() {
                    entries.push(ArchiveEntry {
                        path: entry.path().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
                        size: entry.size(),
                    });
                }
            }
        }
    }

    Ok(entries)
}

/// Extracts an archive over an instance folder. Entry paths are sanitised, the
/// same as world imports: an archive cannot write outside the destination.
pub fn extract<P>(
    archive: &Path,
    destination: &Path,
    cancel: &CancellationToken,
    mut report: P,
) -> AppResult<u64>
where
    P: FnMut(Progress),
{
    let format = Format::from_path(archive).ok_or_else(|| {
        AppError::Other(format!(
            "{} is not a backup archive this build understands",
            archive.display()
        ))
    })?;
    std::fs::create_dir_all(destination).ctx("create the destination", destination)?;

    let file = std::fs::File::open(archive).ctx("open the archive", archive)?;
    let mut written = 0u64;

    match format {
        Format::Zip => {
            let mut zip = zip::ZipArchive::new(file)
                .map_err(|e| AppError::archive(archive.display(), e))?;
            let total = zip.len() as u64;

            for index in 0..zip.len() {
                if cancel.is_cancelled() {
                    return Err(AppError::Cancelled);
                }
                let mut entry = zip
                    .by_index(index)
                    .map_err(|e| AppError::archive(archive.display(), e))?;
                let Some(relative) =
                    crate::worlds::archive::safe_entry_path(entry.name(), None)
                else {
                    continue;
                };
                let target = destination.join(relative);

                if entry.is_dir() {
                    std::fs::create_dir_all(&target).ctx("create folder", &target)?;
                    continue;
                }
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).ctx("create folder", parent)?;
                }
                let mut out = std::fs::File::create(&target).ctx("write file", &target)?;
                written += std::io::copy(&mut entry, &mut out).ctx("write file", &target)?;

                report(Progress {
                    files_done: index as u64 + 1,
                    files_total: total,
                    bytes_read: written,
                });
            }
        }
        Format::TarZst => {
            let decoder = zstd::stream::Decoder::new(file)
                .map_err(|e| AppError::archive(archive.display(), e))?;
            let mut tar = tar::Archive::new(decoder);
            let mut done = 0u64;

            for entry in tar
                .entries()
                .map_err(|e| AppError::archive(archive.display(), e))?
            {
                if cancel.is_cancelled() {
                    return Err(AppError::Cancelled);
                }
                let mut entry =
                    entry.map_err(|e| AppError::archive(archive.display(), e))?;
                let name = entry
                    .path()
                    .map(|path| path.to_string_lossy().to_string())
                    .unwrap_or_default();
                let Some(relative) = crate::worlds::archive::safe_entry_path(&name, None) else {
                    continue;
                };
                let target = destination.join(relative);

                if entry.header().entry_type().is_dir() {
                    std::fs::create_dir_all(&target).ctx("create folder", &target)?;
                    continue;
                }
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).ctx("create folder", parent)?;
                }
                let mut out = std::fs::File::create(&target).ctx("write file", &target)?;
                written += std::io::copy(&mut entry, &mut out).ctx("write file", &target)?;

                done += 1;
                report(Progress {
                    files_done: done,
                    files_total: 0,
                    bytes_read: written,
                });
            }
        }
    }

    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance(root: &Path) -> PathBuf {
        let dir = root.join("server");
        std::fs::create_dir_all(dir.join("world").join("region")).unwrap();
        std::fs::create_dir_all(dir.join("logs")).unwrap();
        std::fs::create_dir_all(dir.join("mods")).unwrap();
        std::fs::create_dir_all(crate::paths::console_dir(&dir)).unwrap();
        std::fs::create_dir_all(dir.join(".msm").join("staging")).unwrap();

        std::fs::write(dir.join("server.properties"), b"motd=hi").unwrap();
        std::fs::write(dir.join("world").join("level.dat"), b"dat").unwrap();
        std::fs::write(dir.join("world").join("region").join("r.0.0.mca"), vec![1u8; 4096]).unwrap();
        std::fs::write(dir.join("world").join("session.lock"), b"lock").unwrap();
        std::fs::write(dir.join("logs").join("latest.log"), vec![2u8; 8192]).unwrap();
        std::fs::write(dir.join("mods").join("a.jar"), vec![3u8; 1024]).unwrap();
        std::fs::write(dir.join(".msm").join("instance.json"), b"{}").unwrap();
        std::fs::write(dir.join(".msm").join("staging").join("half.jar"), b"x").unwrap();
        dir
    }

    fn rel(parts: &[&str]) -> PathBuf {
        parts.iter().collect()
    }

    #[test]
    fn noise_is_never_archived() {
        let worlds = vec!["world".to_string()];
        for path in [
            rel(&["logs", "latest.log"]),
            rel(&["crash-reports", "crash.txt"]),
            rel(&["cache", "x.dat"]),
            rel(&[".msm", "staging", "half.jar"]),
            rel(&[".msm", "console", "console.log"]),
            rel(&["world", "session.lock"]),
            rel(&["debug.log"]),
        ] {
            assert!(
                !include_entry(&path, Scope::Full, &worlds, &[]),
                "{} should be excluded",
                path.display()
            );
        }
    }

    #[test]
    fn the_instance_manifest_is_kept_so_a_restore_can_be_imported() {
        assert!(include_entry(
            &rel(&[".msm", "instance.json"]),
            Scope::Full,
            &[],
            &[]
        ));
    }

    #[test]
    fn a_full_backup_keeps_configuration_and_content() {
        let worlds = vec!["world".to_string()];
        for path in [
            rel(&["server.properties"]),
            rel(&["mods", "a.jar"]),
            rel(&["world", "level.dat"]),
            rel(&["ops.json"]),
        ] {
            assert!(include_entry(&path, Scope::Full, &worlds, &[]));
        }
    }

    #[test]
    fn a_worlds_backup_keeps_only_world_folders() {
        let worlds = vec!["world".to_string(), "creative".to_string()];
        assert!(include_entry(&rel(&["world", "level.dat"]), Scope::Worlds, &worlds, &[]));
        assert!(include_entry(
            &rel(&["creative", "region", "r.0.0.mca"]),
            Scope::Worlds,
            &worlds,
            &[]
        ));
        assert!(!include_entry(&rel(&["server.properties"]), Scope::Worlds, &worlds, &[]));
        assert!(!include_entry(&rel(&["mods", "a.jar"]), Scope::Worlds, &worlds, &[]));
    }

    #[test]
    fn extra_exclusions_take_folder_paths_or_extensions() {
        let worlds = vec!["world".to_string()];
        let extra = vec!["mods".to_string(), "*.tmp".to_string(), "config/private".to_string()];

        assert!(!include_entry(&rel(&["mods", "a.jar"]), Scope::Full, &worlds, &extra));
        assert!(!include_entry(&rel(&["scratch.tmp"]), Scope::Full, &worlds, &extra));
        assert!(!include_entry(
            &rel(&["config", "private", "secrets.yml"]),
            Scope::Full,
            &worlds,
            &extra
        ));
        assert!(include_entry(&rel(&["config", "public.yml"]), Scope::Full, &worlds, &extra));
    }

    #[test]
    fn the_estimate_counts_only_what_would_be_archived() {
        let root = tempfile::tempdir().unwrap();
        let dir = instance(root.path());
        let worlds = vec!["world".to_string()];

        let full = estimate(&dir, Scope::Full, &worlds, &[]).unwrap();
        // level.dat, r.0.0.mca, server.properties, a.jar, instance.json.
        assert_eq!(full.files, 5, "excluded files are not counted");
        assert!(full.bytes >= 4096 + 1024);
        assert!(full.bytes < 4096 + 1024 + 8192, "the log is not counted");

        let worlds_only = estimate(&dir, Scope::Worlds, &worlds, &[]).unwrap();
        assert_eq!(worlds_only.files, 2);
    }

    #[test]
    fn both_formats_round_trip_an_instance() {
        for format in [Format::TarZst, Format::Zip] {
            let root = tempfile::tempdir().unwrap();
            let dir = instance(root.path());
            let worlds = vec!["world".to_string()];
            let archive = root
                .path()
                .join(format!("backup.{}", format.extension()));

            let mut seen = 0;
            let size = write(
                &Spec {
                    instance_dir: dir.clone(),
                    target: archive.clone(),
                    format,
                    level: format.default_level(),
                    scope: Scope::Full,
                    worlds: worlds.clone(),
                    extra: Vec::new(),
                },
                &CancellationToken::new(),
                |progress| seen = progress.files_done,
            )
            .unwrap();

            assert!(size > 0, "{format:?} produced an empty archive");
            assert!(seen > 0, "{format:?} reported no progress");
            assert!(!archive.with_extension("part").exists());

            let listed = list(&archive).unwrap();
            let names: Vec<&str> = listed.iter().map(|entry| entry.path.as_str()).collect();
            assert!(names.contains(&"server.properties"), "{format:?}: {names:?}");
            assert!(names.contains(&"world/level.dat"), "{format:?}: {names:?}");
            assert!(
                !names.iter().any(|name| name.contains("latest.log")),
                "{format:?} archived a log"
            );

            let restored = root.path().join(format!("restored-{}", format.as_str()));
            extract(&archive, &restored, &CancellationToken::new(), |_| {}).unwrap();

            assert_eq!(
                std::fs::read_to_string(restored.join("server.properties")).unwrap(),
                "motd=hi",
                "{format:?}"
            );
            assert_eq!(
                std::fs::metadata(restored.join("world").join("region").join("r.0.0.mca"))
                    .unwrap()
                    .len(),
                4096,
                "{format:?} world data survived"
            );
        }
    }

    #[test]
    fn a_cancelled_backup_leaves_no_archive() {
        let root = tempfile::tempdir().unwrap();
        let dir = instance(root.path());
        let archive = root.path().join("backup.tar.zst");

        let cancel = CancellationToken::new();
        cancel.cancel();
        let err = write(
            &Spec {
                instance_dir: dir,
                target: archive.clone(),
                format: Format::TarZst,
                level: 3,
                scope: Scope::Full,
                worlds: vec!["world".to_string()],
                extra: Vec::new(),
            },
            &cancel,
            |_| {},
        )
        .unwrap_err();

        assert_eq!(err.kind(), "cancelled");
        assert!(!archive.exists());
        assert!(!archive.with_extension("part").exists());
    }

    #[test]
    fn compression_levels_are_clamped_per_format() {
        assert_eq!(Format::TarZst.clamp_level(50), 22);
        assert_eq!(Format::TarZst.clamp_level(0), 1);
        assert_eq!(Format::Zip.clamp_level(50), 9);
        assert_eq!(Format::Zip.clamp_level(-3), 0);
        assert_eq!(Format::TarZst.default_level(), 3);
    }

    #[test]
    fn a_higher_level_produces_a_smaller_archive() {
        let root = tempfile::tempdir().unwrap();
        let dir = instance(root.path());
        // Compressible data, so the levels actually differ.
        std::fs::write(dir.join("world").join("big.dat"), vec![7u8; 512 * 1024]).unwrap();

        let sizes: Vec<u64> = [1, 19]
            .iter()
            .map(|level| {
                let archive = root.path().join(format!("level-{level}.tar.zst"));
                write(
                    &Spec {
                        instance_dir: dir.clone(),
                        target: archive,
                        format: Format::TarZst,
                        level: *level,
                        scope: Scope::Full,
                        worlds: vec!["world".to_string()],
                        extra: Vec::new(),
                    },
                    &CancellationToken::new(),
                    |_| {},
                )
                .unwrap()
            })
            .collect();

        assert!(sizes[1] <= sizes[0], "level 19 should not be larger: {sizes:?}");
    }

    #[test]
    fn the_format_is_recognised_from_the_file_name() {
        assert_eq!(
            Format::from_path(Path::new("backup-2026.tar.zst")),
            Some(Format::TarZst)
        );
        assert_eq!(Format::from_path(Path::new("backup.zip")), Some(Format::Zip));
        assert_eq!(Format::from_path(Path::new("backup.rar")), None);
    }

    #[test]
    fn an_archive_cannot_write_outside_the_destination() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("evil.zip");
        {
            let file = std::fs::File::create(&archive).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            zip.start_file("../escaped.txt", options).unwrap();
            zip.write_all(b"nope").unwrap();
            zip.start_file("kept.txt", options).unwrap();
            zip.write_all(b"fine").unwrap();
            zip.finish().unwrap();
        }

        let destination = root.path().join("restored");
        extract(&archive, &destination, &CancellationToken::new(), |_| {}).unwrap();

        assert!(destination.join("kept.txt").is_file());
        assert!(!root.path().join("escaped.txt").exists());
    }
}
