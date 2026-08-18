//! Exporting a world to a zip and importing one back.
//!
//! Same shape as the downloader: progress is reported per entry, cancellation is
//! checked between entries, and the result only takes its final name once it is
//! complete — a cancelled export leaves no half-written zip pretending to be a
//! backup.

use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use tokio_util::sync::CancellationToken;

use crate::error::{AppError, AppResult, IoContext};

/// Progress for both directions: entries done, entries total (when known), bytes.
#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub entries_done: u64,
    pub entries_total: u64,
    pub bytes: u64,
}

/// Files that are pointless or actively harmful to copy into an archive.
pub fn skip_entry(relative: &Path) -> bool {
    relative
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name == "session.lock")
        .unwrap_or(false)
}

/// Zips a world folder. The archive holds the world folder itself as its root
/// entry, so importing it elsewhere recreates the same folder name.
pub fn export<P>(
    world_dir: &Path,
    target: &Path,
    cancel: &CancellationToken,
    mut report: P,
) -> AppResult<u64>
where
    P: FnMut(Progress),
{
    let root_name = world_dir
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .ok_or_else(|| AppError::Other("the world folder has no name".into()))?;

    let files: Vec<PathBuf> = walkdir::WalkDir::new(world_dir)
        .into_iter()
        .flatten()
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.path().to_path_buf())
        .filter(|path| {
            path.strip_prefix(world_dir)
                .map(|relative| !skip_entry(relative))
                .unwrap_or(false)
        })
        .collect();

    let total = files.len() as u64;
    // Written under a temporary name and renamed at the end.
    let temp = target.with_extension("zip.part");
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).ctx("create export folder", parent)?;
    }

    let file = std::fs::File::create(&temp).ctx("create archive", &temp)?;
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let mut bytes = 0u64;
    let mut done = 0u64;
    let mut buffer = vec![0u8; 64 * 1024];

    for path in files {
        if cancel.is_cancelled() {
            drop(zip);
            let _ = std::fs::remove_file(&temp);
            return Err(AppError::Cancelled);
        }

        let relative = path.strip_prefix(world_dir).unwrap_or(&path);
        // Zip entries always use forward slashes, on every platform.
        let name = format!("{root_name}/{}", to_zip_path(relative));
        zip.start_file(name, options)
            .map_err(|e| AppError::Other(format!("could not add an entry to the archive: {e}")))?;

        let mut source = std::fs::File::open(&path).ctx("read world file", &path)?;
        loop {
            let read = source.read(&mut buffer).ctx("read world file", &path)?;
            if read == 0 {
                break;
            }
            zip.write_all(&buffer[..read])
                .map_err(|e| AppError::Other(format!("could not write the archive: {e}")))?;
            bytes += read as u64;
        }

        done += 1;
        if done % 32 == 0 || done == total {
            report(Progress {
                entries_done: done,
                entries_total: total,
                bytes,
            });
        }
    }

    zip.finish()
        .map_err(|e| AppError::Other(format!("could not finish the archive: {e}")))?;
    std::fs::rename(&temp, target).ctx("finish the archive", target)?;

    report(Progress {
        entries_done: done,
        entries_total: total,
        bytes,
    });
    Ok(bytes)
}

/// Unpacks a world zip into the instance folder.
///
/// Entry paths are sanitised: an archive naming `../../etc/passwd` must not be
/// able to write outside the destination.
pub fn import<P>(
    archive: &Path,
    instance_path: &Path,
    folder_override: Option<&str>,
    cancel: &CancellationToken,
    mut report: P,
) -> AppResult<String>
where
    P: FnMut(Progress),
{
    let file = std::fs::File::open(archive).ctx("open archive", archive)?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| AppError::Other(format!("{} is not a readable zip: {e}", archive.display())))?;

    let total = zip.len() as u64;
    let root = archive_root(&mut zip)?;
    let folder = folder_override
        .map(str::to_string)
        .or_else(|| root.clone())
        .or_else(|| {
            archive
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
        })
        .ok_or_else(|| AppError::Other("could not work out a world folder name".into()))?;

    if folder.contains('/') || folder.contains('\\') || folder.contains("..") {
        return Err(AppError::Other(format!(
            "\"{folder}\" is not a valid world folder name"
        )));
    }

    let destination = instance_path.join(&folder);
    if destination.exists() {
        return Err(AppError::Other(format!(
            "\"{folder}\" already exists in this instance"
        )));
    }

    let mut bytes = 0u64;
    let count = zip.len();
    for index in 0..count {
        if cancel.is_cancelled() {
            let _ = std::fs::remove_dir_all(&destination);
            return Err(AppError::Cancelled);
        }

        let mut entry = zip
            .by_index(index)
            .map_err(|e| AppError::Other(format!("could not read the archive: {e}")))?;

        let Some(relative) = safe_entry_path(entry.name(), root.as_deref()) else {
            continue;
        };
        let target = destination.join(&relative);

        if entry.is_dir() {
            std::fs::create_dir_all(&target).ctx("create folder", &target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).ctx("create folder", parent)?;
        }

        let mut out = std::fs::File::create(&target).ctx("write world file", &target)?;
        bytes += std::io::copy(&mut entry, &mut out).ctx("write world file", &target)? ;

        if index % 32 == 0 || index + 1 == count {
            report(Progress {
                entries_done: index as u64 + 1,
                entries_total: total,
                bytes,
            });
        }
    }

    if !destination.join("level.dat").is_file() {
        let _ = std::fs::remove_dir_all(&destination);
        return Err(AppError::Other(
            "that archive does not contain a world: no level.dat was found".into(),
        ));
    }

    Ok(folder)
}

/// The single top-level folder of an archive, when it has one.
fn archive_root(zip: &mut zip::ZipArchive<std::fs::File>) -> AppResult<Option<String>> {
    let mut root: Option<String> = None;
    for index in 0..zip.len() {
        let entry = zip
            .by_index(index)
            .map_err(|e| AppError::Other(format!("could not read the archive: {e}")))?;
        let name = entry.name().replace('\\', "/");
        let Some(first) = name.split('/').next().filter(|part| !part.is_empty()) else {
            continue;
        };
        match &root {
            None => root = Some(first.to_string()),
            Some(existing) if existing == first => {}
            // More than one top-level entry: the archive is the world itself.
            Some(_) => return Ok(None),
        }
    }
    Ok(root)
}

/// Turns an entry name into a safe relative path, or `None` when it tries to
/// escape.
pub fn safe_entry_path(name: &str, strip_root: Option<&str>) -> Option<PathBuf> {
    let normalised = name.replace('\\', "/");
    let without_root = match strip_root {
        Some(root) => normalised
            .strip_prefix(&format!("{root}/"))
            .unwrap_or(&normalised),
        None => &normalised,
    };

    let mut out = PathBuf::new();
    for part in without_root.split('/') {
        match part {
            "" | "." => continue,
            ".." => return None,
            part if part.contains(':') => return None,
            part => out.push(part),
        }
    }
    // An absolute path in an archive is never legitimate.
    if Path::new(without_root)
        .components()
        .any(|component| matches!(component, Component::RootDir | Component::Prefix(_)))
    {
        return None;
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

fn to_zip_path(relative: &Path) -> String {
    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::super::nbt;
    use super::*;

    fn world_at(root: &Path, folder: &str) -> PathBuf {
        let dir = root.join(folder);
        std::fs::create_dir_all(dir.join("region")).unwrap();
        std::fs::write(
            dir.join("level.dat"),
            nbt::build::gzip(&nbt::build::level_dat(folder, 1, 0, 0)),
        )
        .unwrap();
        std::fs::write(dir.join("region").join("r.0.0.mca"), vec![7u8; 2048]).unwrap();
        std::fs::write(dir.join("session.lock"), b"lock").unwrap();
        dir
    }

    #[test]
    fn a_world_round_trips_through_a_zip() {
        let source = tempfile::tempdir().unwrap();
        let world = world_at(source.path(), "world");
        let archive = source.path().join("world.zip");

        let mut seen = 0;
        let bytes = export(&world, &archive, &CancellationToken::new(), |p| {
            seen = p.entries_done;
        })
        .unwrap();
        assert!(bytes > 2000);
        assert!(archive.is_file());
        assert!(seen > 0, "progress was reported");
        assert!(!archive.with_extension("zip.part").exists());

        let destination = tempfile::tempdir().unwrap();
        let folder = import(
            &archive,
            destination.path(),
            None,
            &CancellationToken::new(),
            |_| {},
        )
        .unwrap();

        assert_eq!(folder, "world");
        let imported = destination.path().join("world");
        assert!(imported.join("level.dat").is_file());
        assert!(imported.join("region").join("r.0.0.mca").is_file());
        assert!(
            !imported.join("session.lock").exists(),
            "the lock file is not carried across"
        );

        // The metadata survives the trip.
        let read = super::super::read_world(&imported, "world");
        assert_eq!(read.display_name.as_deref(), Some("world"));
    }

    #[test]
    fn importing_can_rename_the_world_folder() {
        let source = tempfile::tempdir().unwrap();
        let world = world_at(source.path(), "world");
        let archive = source.path().join("world.zip");
        export(&world, &archive, &CancellationToken::new(), |_| {}).unwrap();

        let destination = tempfile::tempdir().unwrap();
        let folder = import(
            &archive,
            destination.path(),
            Some("restored_world"),
            &CancellationToken::new(),
            |_| {},
        )
        .unwrap();

        assert_eq!(folder, "restored_world");
        assert!(destination.path().join("restored_world").join("level.dat").is_file());
    }

    #[test]
    fn importing_over_an_existing_world_is_refused() {
        let source = tempfile::tempdir().unwrap();
        let world = world_at(source.path(), "world");
        let archive = source.path().join("world.zip");
        export(&world, &archive, &CancellationToken::new(), |_| {}).unwrap();

        let err = import(&archive, source.path(), None, &CancellationToken::new(), |_| {})
            .unwrap_err();
        assert!(err.to_string().contains("already exists"), "{err}");
    }

    #[test]
    fn a_zip_without_a_world_is_rejected_and_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("notaworld.zip");
        {
            let file = std::fs::File::create(&archive).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            zip.start_file("notaworld/readme.txt", options).unwrap();
            zip.write_all(b"nothing to see").unwrap();
            zip.finish().unwrap();
        }

        let destination = tempfile::tempdir().unwrap();
        let err = import(
            &archive,
            destination.path(),
            None,
            &CancellationToken::new(),
            |_| {},
        )
        .unwrap_err();
        assert!(err.to_string().contains("no level.dat"), "{err}");
        assert!(!destination.path().join("notaworld").exists());
    }

    #[test]
    fn a_cancelled_export_leaves_no_archive() {
        let source = tempfile::tempdir().unwrap();
        let world = world_at(source.path(), "world");
        let archive = source.path().join("world.zip");

        let cancel = CancellationToken::new();
        cancel.cancel();
        let err = export(&world, &archive, &cancel, |_| {}).unwrap_err();

        assert_eq!(err.kind(), "cancelled");
        assert!(!archive.exists());
        assert!(!archive.with_extension("zip.part").exists());
    }

    #[test]
    fn entry_paths_that_escape_the_destination_are_refused() {
        assert_eq!(
            safe_entry_path("world/region/r.0.0.mca", Some("world")),
            Some(PathBuf::from("region").join("r.0.0.mca"))
        );
        assert_eq!(safe_entry_path("world/../../etc/passwd", Some("world")), None);
        assert_eq!(safe_entry_path("../escape", None), None);
        assert_eq!(safe_entry_path("/absolute/path", None), None);
        assert_eq!(safe_entry_path("C:/windows/system32", None), None);
        assert_eq!(safe_entry_path("world/", Some("world")), None);
    }

    #[test]
    fn lock_files_are_skipped() {
        assert!(skip_entry(Path::new("session.lock")));
        assert!(skip_entry(&PathBuf::from("region").join("session.lock")));
        assert!(!skip_entry(Path::new("level.dat")));
    }
}
