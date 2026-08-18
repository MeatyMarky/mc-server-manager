//! Minecraft version handling.
//!
//! Two eras exist side by side: the classic `1.MINOR[.PATCH]` line, and the
//! calendar line Mojang moved to (`26.1`, `26.1.2`, `26.2`). Anything that
//! compares or classifies a version has to cope with both, so the parsing lives
//! here rather than being re-invented per provider.

use std::cmp::Ordering;

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

/// Newest first. Unparseable entries drop to the end in their original order.
pub fn sort_newest_first(versions: &mut [String]) {
    versions.sort_by(|a, b| match (parse(a), parse(b)) {
        (Some(x), Some(y)) => compare(&y, &x),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    });
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
    fn sorts_newest_first_across_eras() {
        let mut versions: Vec<String> = ["1.20.4", "26.2", "1.21.4", "26.1.2", "nonsense"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        sort_newest_first(&mut versions);
        assert_eq!(
            versions,
            vec!["26.2", "26.1.2", "1.21.4", "1.20.4", "nonsense"]
        );
    }
}
