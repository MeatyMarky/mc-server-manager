//! Project icons, cached on disk.
//!
//! A grid of twenty cards means twenty images, and scrolling back and forth
//! would fetch each one again on every render. They land in
//! `<data>/cache/icons/` under a name derived from the URL, so a second request
//! for the same icon is a file read — and a card with no icon at all is a
//! placeholder the UI draws, never a broken image.

use std::path::{Path, PathBuf};

use sha2::Digest as _;

use crate::error::{AppError, AppResult, IoContext};
use crate::http::Http;

/// Nothing larger than this is an icon; something else is going on.
const MAX_ICON_BYTES: u64 = 4 * 1024 * 1024;

pub fn icons_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("cache").join("icons")
}

/// Where one icon lives.
///
/// Named after a hash of the URL: icon URLs carry their own ids and CDN paths,
/// and hashing keeps a file name that is valid on both platforms whatever the
/// source puts in a URL.
pub fn icon_path(data_dir: &Path, url: &str) -> PathBuf {
    let digest = sha2::Sha256::digest(url.as_bytes());
    let name = hex::encode(&digest[..16]);
    icons_dir(data_dir).join(format!("{name}.{}", extension_of(url)))
}

/// The image type, from the URL. Defaults to png, which every source uses.
fn extension_of(url: &str) -> &str {
    let tail = url.rsplit('/').next().unwrap_or_default();
    let stem = tail.split(['?', '#']).next().unwrap_or_default();
    match stem.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()) {
        Some(ext) if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp") => {
            match ext.as_str() {
                "png" => "png",
                "jpg" => "jpg",
                "jpeg" => "jpeg",
                "gif" => "gif",
                _ => "webp",
            }
        }
        _ => "png",
    }
}

/// The cached file for an icon, fetching it once if this is the first time.
///
/// Returns `None` for a project with no icon, which is a normal state and not a
/// failure: the card draws a placeholder.
pub async fn ensure_cached(http: &Http, data_dir: &Path, url: Option<&str>) -> AppResult<Option<PathBuf>> {
    let Some(url) = url.map(str::trim).filter(|url| !url.is_empty()) else {
        return Ok(None);
    };
    if !url.starts_with("https://") {
        // Icons come from the sources' own CDNs. Anything else is not something
        // to fetch on a card's behalf.
        return Ok(None);
    }

    let target = icon_path(data_dir, url);
    if target.is_file() {
        return Ok(Some(target));
    }

    let dir = icons_dir(data_dir);
    tokio::fs::create_dir_all(&dir)
        .await
        .ctx("create the icon cache", &dir)?;

    let response = http
        .client()
        .get(url)
        .send()
        .await
        .map_err(|e| crate::error::from_reqwest(url, &e))?;
    if !response.status().is_success() {
        return Err(AppError::Network(format!(
            "{url} returned {}",
            response.status()
        )));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_ICON_BYTES)
    {
        return Err(AppError::Other(format!("{url} is too large for an icon")));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| AppError::Network(format!("{url} could not be read: {e}")))?;
    if bytes.len() as u64 > MAX_ICON_BYTES {
        return Err(AppError::Other(format!("{url} is too large for an icon")));
    }

    // Written beside the target and renamed, so a cancelled fetch never leaves
    // a half-written image that later reads as cached.
    let partial = target.with_extension("part");
    tokio::fs::write(&partial, &bytes)
        .await
        .ctx("write the icon", &partial)?;
    tokio::fs::rename(&partial, &target)
        .await
        .ctx("store the icon", &target)?;

    Ok(Some(target))
}

/// Bytes on disk, for the Settings figure.
pub fn cache_size(data_dir: &Path) -> u64 {
    walkdir::WalkDir::new(icons_dir(data_dir))
        .into_iter()
        .flatten()
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| entry.metadata().ok())
        .map(|meta| meta.len())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_icon_is_named_after_its_url_and_keeps_its_type() {
        let data = Path::new("/data");
        let png = icon_path(data, "https://cdn.modrinth.com/data/AANobbMI/icon.png");
        let jpg = icon_path(data, "https://media.forgecdn.net/avatars/29/69/635638176075724951.jpg");

        assert!(png.starts_with(icons_dir(data)));
        assert_eq!(png.extension().unwrap(), "png");
        assert_eq!(jpg.extension().unwrap(), "jpg");

        // The same URL always lands on the same file, which is what makes the
        // second request a file read.
        assert_eq!(
            png,
            icon_path(data, "https://cdn.modrinth.com/data/AANobbMI/icon.png")
        );
        // Different URLs never collide.
        assert_ne!(
            png,
            icon_path(data, "https://cdn.modrinth.com/data/OTHER/icon.png")
        );
    }

    #[test]
    fn a_url_with_a_query_or_an_odd_ending_still_produces_a_usable_name() {
        let data = Path::new("/data");
        let queried = icon_path(data, "https://example.com/icon.png?width=64");
        assert_eq!(queried.extension().unwrap(), "png");

        // No extension at all, and a CDN path with characters a file name
        // cannot hold: hashing means neither is a problem.
        let odd = icon_path(data, "https://example.com/a:b|c/icon");
        assert_eq!(odd.extension().unwrap(), "png");
        let name = odd.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.'), "{name}");
    }

    #[tokio::test]
    async fn a_project_without_an_icon_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let http = Http::default_client();

        assert!(ensure_cached(&http, dir.path(), None).await.unwrap().is_none());
        assert!(ensure_cached(&http, dir.path(), Some("")).await.unwrap().is_none());
        // Nothing is fetched for a URL that is not an https CDN link.
        assert!(ensure_cached(&http, dir.path(), Some("file:///etc/passwd"))
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn an_already_cached_icon_is_not_fetched_again() {
        let dir = tempfile::tempdir().unwrap();
        let http = Http::default_client();
        let url = "https://cdn.example.com/icon.png";

        // Pretend it was fetched earlier.
        let target = icon_path(dir.path(), url);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, b"not really a png").unwrap();

        // No network is available in tests; this returning the path at all
        // proves nothing was requested.
        let cached = ensure_cached(&http, dir.path(), Some(url)).await.unwrap();
        assert_eq!(cached, Some(target));
        assert!(cache_size(dir.path()) > 0);
    }
}
