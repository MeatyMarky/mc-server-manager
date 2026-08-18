//! Minecraft version handling.
//!
//! Two eras exist side by side: the classic `1.MINOR[.PATCH]` line, and the
//! calendar line Mojang moved to (`26.1`, `26.1.2`, `26.2`). Anything that
//! compares or classifies a version has to cope with both, so the parsing lives
//! here rather than being re-invented per provider.

use std::cmp::Ordering;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Era {
    /// `1.x` releases, up to and including the 1.21 line.
    Classic,
    /// Calendar-numbered releases: `26.1`, `26.2`, …
    Calendar,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McVersion {
    pub era: Era,
    /// Numeric components, e.g. `1.21.4` -> [1, 21, 4], `26.1.2` -> [26, 1, 2].
    pub parts: Vec<u32>,
    /// Anything after the numeric prefix: `-pre1`, `-rc2`, `-snapshot-9`.
    pub suffix: Option<String>,
    pub raw: String,
}

impl McVersion {
    pub fn is_release(&self) -> bool {
        self.suffix.is_none()
    }

    fn part(&self, index: usize) -> u32 {
        self.parts.get(index).copied().unwrap_or(0)
    }
}

/// Parses a release or pre-release identifier. Returns `None` for things that
/// are not versions at all (`"latest"`, `""`, a jar name).
pub fn parse(raw: &str) -> Option<McVersion> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Split the numeric prefix from any suffix: 1.21.4-pre2, 26.3-snapshot-9.
    let split_at = trimmed
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(trimmed.len());
    let (numeric, rest) = trimmed.split_at(split_at);
    let numeric = numeric.trim_end_matches('.');
    if numeric.is_empty() {
        return None;
    }

    let mut parts = Vec::new();
    for piece in numeric.split('.') {
        parts.push(piece.parse::<u32>().ok()?);
    }
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }

    // Old snapshots ("13w41a") never parse here, which is intentional: they are
    // not supported as instance versions.
    let era = if parts[0] == 1 { Era::Classic } else { Era::Calendar };
    if era == Era::Calendar && parts[0] < 20 {
        return None;
    }

    Some(McVersion {
        era,
        parts,
        suffix: (!rest.is_empty()).then(|| rest.trim_start_matches('-').to_string()),
        raw: trimmed.to_string(),
    })
}

/// True when the string looks like a Minecraft version in either era.
pub fn looks_like_version(raw: &str) -> bool {
    parse(raw).is_some()
}

/// Orders two versions. A release sorts after its own pre-releases.
pub fn compare(a: &McVersion, b: &McVersion) -> Ordering {
    for index in 0..4 {
        match a.part(index).cmp(&b.part(index)) {
            Ordering::Equal => continue,
            other => return other,
        }
    }
    match (&a.suffix, &b.suffix) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => x.cmp(y),
    }
}

/// `a >= b`, for readable comparisons at call sites.
pub fn at_least(a: &str, b: &str) -> bool {
    match (parse(a), parse(b)) {
        (Some(a), Some(b)) => compare(&a, &b) != Ordering::Less,
        _ => false,
    }
}

/// Newest first, using *only* the parsed components.
///
/// This is the fallback for versions Mojang's manifest does not know about.
/// Anything user-visible sorts through [`VersionIndex`] instead, because release
/// chronology is the real ordering and version strings only approximate it.
pub fn sort_newest_first_by_components(versions: &mut [String]) {
    versions.sort_by(|a, b| match (parse(a), parse(b)) {
        (Some(x), Some(y)) => compare(&y, &x),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    });
}

/// One entry of Mojang's version manifest, as far as ordering cares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedVersion {
    pub id: String,
    /// RFC3339, fixed width, so string comparison is chronological.
    pub release_time: String,
    pub kind: String,
    /// Position in the manifest; 0 is the newest entry.
    pub position: i64,
}

/// Release chronology, straight from Mojang.
///
/// `1.21.11` released before `26.2`, and no amount of component parsing proves
/// that — the two numbering schemes are not comparable. Every user-visible sort
/// (version pickers, "is there a newer build", mod filtering) goes through here.
/// Versions the manifest does not list (a Paper release candidate, a hand-typed
/// snapshot) fall back to component ordering and always sort *after* anything
/// the manifest does know, because an unknown id cannot be placed in time.
#[derive(Debug, Clone, Default)]
pub struct VersionIndex {
    by_id: HashMap<String, IndexedVersion>,
}

impl VersionIndex {
    pub fn from_entries(entries: impl IntoIterator<Item = IndexedVersion>) -> Self {
        Self {
            by_id: entries
                .into_iter()
                .map(|entry| (entry.id.clone(), entry))
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn get(&self, id: &str) -> Option<&IndexedVersion> {
        self.by_id.get(id)
    }

    pub fn release_time(&self, id: &str) -> Option<&str> {
        self.by_id.get(id).map(|entry| entry.release_time.as_str())
    }

    /// True when `a` was released after `b`.
    pub fn is_newer(&self, a: &str, b: &str) -> bool {
        self.compare(a, b) == Ordering::Greater
    }

    /// Chronological ordering: `Greater` means "released later".
    pub fn compare(&self, a: &str, b: &str) -> Ordering {
        match (self.by_id.get(a), self.by_id.get(b)) {
            (Some(x), Some(y)) => x
                .release_time
                .cmp(&y.release_time)
                // Same timestamp: manifest position decides, 0 being newest.
                .then_with(|| y.position.cmp(&x.position)),
            // A version Mojang knows always outranks one it does not.
            (Some(_), None) => Ordering::Greater,
            (None, Some(_)) => Ordering::Less,
            (None, None) => match (parse(a), parse(b)) {
                (Some(x), Some(y)) => compare(&x, &y),
                (Some(_), None) => Ordering::Greater,
                (None, Some(_)) => Ordering::Less,
                (None, None) => Ordering::Equal,
            },
        }
    }

    /// Newest release first.
    pub fn sort_newest_first(&self, versions: &mut [String]) {
        versions.sort_by(|a, b| self.compare(b, a));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_eras() {
        let classic = parse("1.21.4").unwrap();
        assert_eq!(classic.era, Era::Classic);
        assert_eq!(classic.parts, vec![1, 21, 4]);
        assert!(classic.is_release());

        let calendar = parse("26.1.2").unwrap();
        assert_eq!(calendar.era, Era::Calendar);
        assert_eq!(calendar.parts, vec![26, 1, 2]);

        let short = parse("26.2").unwrap();
        assert_eq!(short.parts, vec![26, 2]);
    }

    #[test]
    fn parses_pre_releases_and_snapshots() {
        let pre = parse("1.21.11-pre3").unwrap();
        assert_eq!(pre.parts, vec![1, 21, 11]);
        assert_eq!(pre.suffix.as_deref(), Some("pre3"));
        assert!(!pre.is_release());

        let snapshot = parse("26.3-snapshot-9").unwrap();
        assert_eq!(snapshot.parts, vec![26, 3]);
        assert!(!snapshot.is_release());
    }

    #[test]
    fn rejects_things_that_are_not_versions() {
        assert!(parse("").is_none());
        assert!(parse("latest").is_none());
        assert!(parse("13w41a").is_none());
        assert!(parse("server.jar").is_none());
        // Old two-digit-lead numbers that predate the calendar era.
        assert!(parse("12.4").is_none());
    }

    #[test]
    fn orders_within_and_across_eras() {
        assert!(at_least("1.21.4", "1.21"));
        assert!(at_least("1.21", "1.20.6"));
        assert!(!at_least("1.20.4", "1.20.5"));
        // The calendar era is newer than every 1.x release.
        assert!(at_least("26.1", "1.21.11"));
        assert!(at_least("26.2", "26.1.2"));
        assert!(!at_least("26.1", "26.2"));
    }

    #[test]
    fn a_release_outranks_its_own_pre_release() {
        let release = parse("1.21.11").unwrap();
        let pre = parse("1.21.11-pre3").unwrap();
        assert_eq!(compare(&release, &pre), Ordering::Greater);
    }

    #[test]
    fn component_sorting_is_only_the_fallback() {
        let mut versions: Vec<String> = ["1.20.4", "26.2", "1.21.4", "26.1.2", "nonsense"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        sort_newest_first_by_components(&mut versions);
        assert_eq!(
            versions,
            vec!["26.2", "26.1.2", "1.21.4", "1.20.4", "nonsense"]
        );
    }

    fn entry(id: &str, release_time: &str, position: i64) -> IndexedVersion {
        IndexedVersion {
            id: id.to_string(),
            release_time: release_time.to_string(),
            kind: "release".to_string(),
            position,
        }
    }

    /// Release timestamps in the shape Mojang publishes them, newest first.
    fn index() -> VersionIndex {
        VersionIndex::from_entries([
            entry("26.2", "2026-08-04T10:00:00+00:00", 0),
            entry("26.1.2", "2026-06-16T09:00:00+00:00", 1),
            entry("26.1", "2026-05-27T11:00:00+00:00", 2),
            entry("1.21.11", "2026-03-10T12:00:00+00:00", 3),
            entry("1.21.4", "2024-12-03T10:12:57+00:00", 4),
            entry("1.20.4", "2023-12-07T12:56:20+00:00", 5),
        ])
    }

    #[test]
    fn ordering_follows_release_chronology_not_the_version_string() {
        let index = index();
        // The point of the index: 26.2 is newer than 1.21.11 because Mojang
        // released it later, not because 26 > 1.
        assert!(index.is_newer("26.2", "1.21.11"));
        assert!(index.is_newer("1.21.11", "1.21.4"));
        assert!(!index.is_newer("1.21.4", "26.1"));
    }

    #[test]
    fn index_sorting_puts_the_newest_release_first() {
        let index = index();
        let mut versions: Vec<String> = ["1.21.4", "26.1", "1.20.4", "26.2", "1.21.11"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        index.sort_newest_first(&mut versions);
        assert_eq!(versions, vec!["26.2", "26.1", "1.21.11", "1.21.4", "1.20.4"]);
    }

    #[test]
    fn unknown_versions_sort_after_known_ones_and_fall_back_to_components() {
        let index = index();
        let mut versions: Vec<String> = ["1.21.4", "9.9.9-custom", "26.2", "8.8.8-custom"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        index.sort_newest_first(&mut versions);
        assert_eq!(&versions[..2], &["26.2".to_string(), "1.21.4".to_string()]);
        // Between two unknowns, components still give a stable, sensible order.
        assert_eq!(
            &versions[2..],
            &["9.9.9-custom".to_string(), "8.8.8-custom".to_string()]
        );
    }

    #[test]
    fn an_empty_index_degrades_to_component_ordering() {
        let index = VersionIndex::default();
        assert!(index.is_empty());
        let mut versions: Vec<String> = ["1.20.4", "26.2", "1.21.4"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        index.sort_newest_first(&mut versions);
        assert_eq!(versions, vec!["26.2", "1.21.4", "1.20.4"]);
    }

    #[test]
    fn a_shared_timestamp_falls_back_to_manifest_position() {
        let index = VersionIndex::from_entries([
            entry("a", "2026-01-01T00:00:00+00:00", 0),
            entry("b", "2026-01-01T00:00:00+00:00", 1),
        ]);
        assert!(index.is_newer("a", "b"), "position 0 is the newer entry");
    }
}
