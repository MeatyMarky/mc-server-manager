//! Resumable, cancellable downloads with checksum verification.
//!
//! Rules that keep a half-downloaded 200 MB installer from ever being mistaken
//! for a complete one:
//!   * bytes land in `<file>.part`, never at the final path;
//!   * a resumed transfer only appends when the server honours `Range`, and
//!     restarts from zero otherwise;
//!   * the checksum is verified before the rename, and a mismatch deletes the
//!     partial file;
//!   * the rename to the final name is the last step, so the final path only
//!     ever exists complete and verified.

use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use md5::Digest as _;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use crate::error::{AppError, AppResult, IoContext};
use crate::http::Http;
use crate::providers::Artifact;

/// Which digest a provider published for an artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algorithm {
    /// Modrinth publishes SHA-512 for every file in a pack.
    Sha512,
    Sha256,
    Sha1,
    Md5,
}

impl Algorithm {
    pub fn as_str(self) -> &'static str {
        match self {
            Algorithm::Sha512 => "SHA-512",
            Algorithm::Sha256 => "SHA-256",
            Algorithm::Sha1 => "SHA-1",
            Algorithm::Md5 => "MD5",
        }
    }
}

/// The strongest checksum the artifact carries, if any.
pub fn expected_checksum(artifact: &Artifact) -> Option<(Algorithm, String)> {
    if let Some(sha512) = &artifact.sha512 {
        return Some((Algorithm::Sha512, sha512.to_ascii_lowercase()));
    }
    if let Some(sha256) = &artifact.sha256 {
        return Some((Algorithm::Sha256, sha256.to_ascii_lowercase()));
    }
    if let Some(sha1) = &artifact.sha1 {
        return Some((Algorithm::Sha1, sha1.to_ascii_lowercase()));
    }
    artifact
        .md5
        .as_ref()
        .map(|md5| (Algorithm::Md5, md5.to_ascii_lowercase()))
}

/// `<file>.part`, the only place bytes are written before verification.
pub fn part_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    target.with_file_name(name)
}

pub fn hash_file_sync(path: &Path, algorithm: Algorithm) -> AppResult<String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path).ctx("open file", path)?;
    let mut buffer = vec![0u8; 1024 * 1024];

    let mut sha512 = sha2::Sha512::new();
    let mut sha256 = sha2::Sha256::new();
    let mut sha1 = sha1::Sha1::new();
    let mut md5 = md5::Md5::new();

    loop {
        let read = file.read(&mut buffer).ctx("read file", path)?;
        if read == 0 {
            break;
        }
        match algorithm {
            Algorithm::Sha512 => sha512.update(&buffer[..read]),
            Algorithm::Sha256 => sha256.update(&buffer[..read]),
            Algorithm::Sha1 => sha1.update(&buffer[..read]),
            Algorithm::Md5 => md5.update(&buffer[..read]),
        }
    }

    Ok(match algorithm {
        Algorithm::Sha512 => hex::encode(sha512.finalize()),
        Algorithm::Sha256 => hex::encode(sha256.finalize()),
        Algorithm::Sha1 => hex::encode(sha1.finalize()),
        Algorithm::Md5 => hex::encode(md5.finalize()),
    })
}

pub async fn hash_file(path: &Path, algorithm: Algorithm) -> AppResult<String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || hash_file_sync(&path, algorithm))
        .await
        .map_err(|e| AppError::Other(format!("hashing task failed: {e}")))?
}

/// Compares a computed digest with the published one, case-insensitively.
pub fn checksum_matches(expected: &str, actual: &str) -> bool {
    expected.trim().eq_ignore_ascii_case(actual.trim())
}

/// Progress callback payload. The caller turns this into a `task://progress`
/// event; the download engine itself knows nothing about Tauri.
#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub downloaded: u64,
    pub total: Option<u64>,
    pub resumed: bool,
}

/// Downloads `artifact` to `target`, resuming an interrupted `.part` file when
/// the server supports ranges. Returns the number of bytes transferred in this
/// call (zero when the verified file was already present).
pub async fn download<P>(
    http: &Http,
    artifact: &Artifact,
    target: &Path,
    cancel: &CancellationToken,
    mut on_progress: P,
) -> AppResult<u64>
where
    P: FnMut(Progress) + Send,
{
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .ctx("create download folder", parent)?;
    }

    // An existing final file is reused only if it still verifies.
    if target.is_file() && verify(target, artifact).await.is_ok() {
        tracing::debug!(path = %target.display(), "reusing verified cached artifact");
        return Ok(0);
    }

    let part = part_path(target);
    let already = match tokio::fs::metadata(&part).await {
        Ok(meta) if meta.is_file() => meta.len(),
        _ => 0,
    };

    let mut request = http.client().get(&artifact.url);
    if already > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={already}-"));
    }

    let response = request
        .send()
        .await
        .map_err(|e| AppError::Network(format!("{} could not be reached: {e}", artifact.url)))?;

    let status = response.status();
    if !status.is_success() {
        return Err(AppError::Network(format!(
            "{} answered {} {}",
            artifact.url,
            status.as_u16(),
            status.canonical_reason().unwrap_or("")
        )));
    }

    // 206 means the range was honoured; anything else restarts from zero.
    let resumed = already > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT;
    let mut downloaded = if resumed { already } else { 0 };
    let total = response
        .content_length()
        .map(|len| len + downloaded)
        .or(artifact.size);

    let mut file = if resumed {
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(&part)
            .await
            .ctx("open partial download", &part)?
    } else {
        tokio::fs::File::create(&part)
            .await
            .ctx("create partial download", &part)?
    };

    on_progress(Progress {
        downloaded,
        total,
        resumed,
    });

    let mut stream = response.bytes_stream();
    let mut since_report = 0u64;

    while let Some(chunk) = tokio::select! {
        biased;
        _ = cancel.cancelled() => None,
        chunk = stream.next() => chunk,
    } {
        if cancel.is_cancelled() {
            break;
        }
        let chunk = chunk
            .map_err(|e| AppError::Network(format!("transfer of {} failed: {e}", artifact.url)))?;
        file.write_all(&chunk)
            .await
            .ctx("write partial download", &part)?;
        downloaded += chunk.len() as u64;
        since_report += chunk.len() as u64;

        // Roughly every 256 KB, so the UI moves without flooding the IPC bridge.
        if since_report >= 256 * 1024 {
            since_report = 0;
            on_progress(Progress {
                downloaded,
                total,
                resumed,
            });
        }
    }

    file.flush().await.ctx("flush partial download", &part)?;
    drop(file);

    if cancel.is_cancelled() {
        // The .part file stays on disk on purpose: the next attempt resumes it.
        return Err(AppError::Cancelled);
    }

    on_progress(Progress {
        downloaded,
        total,
        resumed,
    });

    verify(&part, artifact).await.inspect_err(|_| {
        // A corrupt partial must never be resumed into a "complete" file.
        let _ = std::fs::remove_file(&part);
    })?;

    tokio::fs::rename(&part, target)
        .await
        .ctx("finish download", target)?;
    Ok(downloaded)
}

/// Verifies a file against whatever checksum the provider published. Providers
/// that publish none (Fabric's generated launcher) pass by definition.
pub async fn verify(path: &Path, artifact: &Artifact) -> AppResult<()> {
    let Some((algorithm, expected)) = expected_checksum(artifact) else {
        return Ok(());
    };
    let actual = hash_file(path, algorithm).await?;
    if checksum_matches(&expected, &actual) {
        return Ok(());
    }
    Err(AppError::ChecksumMismatch {
        file: artifact.file_name.clone(),
        algorithm: algorithm.as_str(),
        expected,
        actual,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ArtifactKind;

    fn artifact() -> Artifact {
        Artifact {
            url: "https://example.invalid/server.jar".into(),
            file_name: "server.jar".into(),
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
    fn partial_downloads_get_a_part_suffix() {
        let target = PathBuf::from("downloads").join("forge-installer.jar");
        assert_eq!(
            part_path(&target),
            PathBuf::from("downloads").join("forge-installer.jar.part")
        );
    }

    #[test]
    fn the_strongest_published_checksum_wins() {
        let mut a = artifact();
        a.md5 = Some("aaaa".into());
        assert_eq!(expected_checksum(&a).unwrap().0, Algorithm::Md5);
        a.sha1 = Some("BBBB".into());
        assert_eq!(expected_checksum(&a).unwrap().0, Algorithm::Sha1);
        a.sha256 = Some("CCCC".into());
        let (algorithm, value) = expected_checksum(&a).unwrap();
        assert_eq!(algorithm, Algorithm::Sha256);
        assert_eq!(value, "cccc", "comparison is case-insensitive");
    }

    #[test]
    fn checksums_compare_case_insensitively() {
        assert!(checksum_matches("ABCDEF", "abcdef"));
        assert!(!checksum_matches("abcdef", "abcde0"));
    }

    #[test]
    fn hashes_match_known_vectors() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("payload.bin");
        std::fs::write(&file, b"abc").unwrap();

        assert_eq!(
            hash_file_sync(&file, Algorithm::Sha1).unwrap(),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hash_file_sync(&file, Algorithm::Sha256).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hash_file_sync(&file, Algorithm::Md5).unwrap(),
            "900150983cd24fb0d6963f7d28e17f72"
        );
    }

    #[test]
    fn md5_handles_multi_block_input() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("long.bin");
        // 1 000 000 'a' characters: the standard MD5 test vector.
        std::fs::write(&file, "a".repeat(1_000_000)).unwrap();
        assert_eq!(
            hash_file_sync(&file, Algorithm::Md5).unwrap(),
            "7707d6ae4e027c70eea2a935c2296f21"
        );
    }

    #[tokio::test]
    async fn verification_rejects_a_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("server.jar");
        std::fs::write(&file, b"not the real jar").unwrap();

        let mut a = artifact();
        a.sha1 = Some("a9993e364706816aba3e25717850c26c9cd0d89d".into());
        let err = verify(&file, &a).await.unwrap_err();
        assert_eq!(err.kind(), "checksum_mismatch");
        assert!(err.to_string().contains("server.jar is corrupt"));
    }

    #[tokio::test]
    async fn artifacts_without_a_checksum_verify_trivially() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("fabric.jar");
        std::fs::write(&file, b"anything").unwrap();
        assert!(verify(&file, &artifact()).await.is_ok());
    }
}
