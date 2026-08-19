//! Where to look for Java, and how to confirm a candidate really is one.
//!
//! Candidate gathering is deliberately small: PATH, `JAVA_HOME`, `CLASSPATH`
//! entries, the standard install roots for the platform, and the Windows
//! registry. Confirmation is always `java -version`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::version::{self, JavaVersionInfo};
use super::JavaSource;

/// What the cache remembers about a binary, so an unchanged one is not re-run.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CachedProbe {
    pub path: String,
    pub mtime: Option<i64>,
    pub size_bytes: Option<i64>,
    pub major: i64,
    pub full_version: Option<String>,
    pub vendor: Option<String>,
    pub arch: Option<String>,
    pub bits: Option<i64>,
    pub valid: bool,
}

#[derive(Debug, Clone)]
pub struct Probe {
    pub path: String,
    pub source: JavaSource,
    pub major: Option<i64>,
    pub full_version: Option<String>,
    pub vendor: Option<String>,
    pub arch: Option<String>,
    /// 64 or 32, as the JVM reported it.
    pub bits: Option<i64>,
    pub mtime: Option<i64>,
    pub size_bytes: Option<i64>,
    pub error: Option<String>,
}

pub fn java_executable_name() -> &'static str {
    if cfg!(windows) {
        "java.exe"
    } else {
        "java"
    }
}

/// Accepts a JDK home, a `bin` folder, or the binary itself, and returns the
/// binary. Users browsing for a JDK point at any of the three.
pub fn java_binary_in(candidate: &Path) -> Option<PathBuf> {
    if candidate.is_file() {
        return Some(candidate.to_path_buf());
    }
    let direct = candidate.join(java_executable_name());
    if direct.is_file() {
        return Some(direct);
    }
    let in_bin = candidate.join("bin").join(java_executable_name());
    in_bin.is_file().then_some(in_bin)
}

/// Standard install roots per platform. Each entry is scanned one level deep
/// for JDK homes.
pub fn install_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();

    if cfg!(windows) {
        for env_key in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
            if let Ok(base) = std::env::var(env_key) {
                let base = PathBuf::from(base);
                roots.push(base.join("Java"));
                roots.push(base.join("Eclipse Adoptium"));
                roots.push(base.join("Eclipse Foundation"));
                roots.push(base.join("Microsoft"));
                roots.push(base.join("Amazon Corretto"));
                roots.push(base.join("Zulu"));
                roots.push(base.join("BellSoft"));
            }
        }
    } else {
        roots.push(PathBuf::from("/usr/lib/jvm"));
        roots.push(PathBuf::from("/usr/java"));
        roots.push(PathBuf::from("/opt/java"));
        roots.push(PathBuf::from("/opt/jdk"));
        roots.push(PathBuf::from("/Library/Java/JavaVirtualMachines"));
        if let Ok(home) = std::env::var("HOME") {
            let home = PathBuf::from(home);
            roots.push(home.join(".sdkman/candidates/java"));
            roots.push(home.join(".jdks"));
        }
    }

    roots
}

/// Every candidate binary worth probing, deduplicated, with where it came from.
pub fn candidates() -> Vec<(PathBuf, JavaSource)> {
    let mut found: Vec<(PathBuf, JavaSource)> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    let push = |path: PathBuf, source: JavaSource, found: &mut Vec<_>, seen: &mut BTreeSet<String>| {
        let key = path.to_string_lossy().to_ascii_lowercase();
        if path.is_file() && seen.insert(key) {
            found.push((path, source));
        }
    };

    // JAVA_HOME first: it is what a user explicitly configured.
    if let Ok(java_home) = std::env::var("JAVA_HOME") {
        if let Some(binary) = java_binary_in(Path::new(&java_home)) {
            push(binary, JavaSource::JavaHome, &mut found, &mut seen);
        }
    }

    // PATH.
    if let Ok(path_var) = std::env::var("PATH") {
        for entry in std::env::split_paths(&path_var) {
            let binary = entry.join(java_executable_name());
            if binary.is_file() {
                push(binary, JavaSource::Path, &mut found, &mut seen);
            }
        }
    }

    // CLASSPATH entries occasionally point inside a JDK; walk up from each entry.
    if let Ok(classpath) = std::env::var("CLASSPATH") {
        for entry in std::env::split_paths(&classpath) {
            let mut cursor = entry.as_path();
            for _ in 0..3 {
                if let Some(binary) = java_binary_in(cursor) {
                    push(binary, JavaSource::Classpath, &mut found, &mut seen);
                    break;
                }
                match cursor.parent() {
                    Some(parent) => cursor = parent,
                    None => break,
                }
            }
        }
    }

    // Standard install roots, one level deep.
    for root in install_roots() {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Some(binary) = java_binary_in(&entry.path()) {
                push(binary, JavaSource::CommonDir, &mut found, &mut seen);
            }
        }
    }

    for home in registry_java_homes() {
        if let Some(binary) = java_binary_in(&home) {
            push(binary, JavaSource::Registry, &mut found, &mut seen);
        }
    }

    found
}

/// Windows registry: the JavaSoft keys plus the Adoptium/Temurin layout.
#[cfg(windows)]
pub fn registry_java_homes() -> Vec<PathBuf> {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY};
    use winreg::RegKey;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let mut homes = Vec::new();

    // JavaSoft: <key>\<version>\JavaHome
    for key_path in [
        r"SOFTWARE\JavaSoft\JDK",
        r"SOFTWARE\JavaSoft\Java Development Kit",
        r"SOFTWARE\JavaSoft\JRE",
        r"SOFTWARE\JavaSoft\Java Runtime Environment",
    ] {
        let Ok(key) = hklm.open_subkey_with_flags(key_path, KEY_READ | KEY_WOW64_64KEY) else {
            continue;
        };
        for version in key.enum_keys().flatten() {
            if let Ok(sub) = key.open_subkey_with_flags(&version, KEY_READ | KEY_WOW64_64KEY) {
                if let Ok(home) = sub.get_value::<String, _>("JavaHome") {
                    homes.push(PathBuf::from(home));
                }
            }
        }
    }

    // Adoptium/Temurin: <vendor>\JDK\<version>\hotspot\MSI\Path
    for vendor_key in [r"SOFTWARE\Eclipse Adoptium", r"SOFTWARE\Eclipse Foundation"] {
        let Ok(vendor) = hklm.open_subkey_with_flags(vendor_key, KEY_READ | KEY_WOW64_64KEY) else {
            continue;
        };
        for product in vendor.enum_keys().flatten() {
            let Ok(product_key) = vendor.open_subkey_with_flags(&product, KEY_READ | KEY_WOW64_64KEY)
            else {
                continue;
            };
            for version in product_key.enum_keys().flatten() {
                let msi = format!(r"{version}\hotspot\MSI");
                if let Ok(msi_key) = product_key.open_subkey_with_flags(&msi, KEY_READ | KEY_WOW64_64KEY) {
                    if let Ok(path) = msi_key.get_value::<String, _>("Path") {
                        homes.push(PathBuf::from(path));
                    }
                }
            }
        }
    }

    homes
}

#[cfg(not(windows))]
pub fn registry_java_homes() -> Vec<PathBuf> {
    Vec::new()
}

/// Runs `java -version` and reads the answer. Blocking: callers wrap it in
/// `spawn_blocking`.
pub fn probe(binary: &Path, source: JavaSource) -> Probe {
    let (mtime, size_bytes) = file_stamp(binary);
    let mut probe = Probe {
        path: binary.to_string_lossy().to_string(),
        source,
        major: None,
        full_version: None,
        vendor: None,
        arch: None,
        bits: None,
        mtime,
        size_bytes,
        error: None,
    };

    let mut command = std::process::Command::new(binary);
    command.arg("-version");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    match command.output() {
        Ok(output) => {
            // java -version writes to stderr; some builds also echo on stdout.
            let text = format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            );
            match version::parse_java_version(&text) {
                Some(JavaVersionInfo {
                    major,
                    full,
                    vendor,
                    bits,
                    ..
                }) => {
                    probe.major = Some(major);
                    probe.full_version = Some(full);
                    probe.vendor = vendor;
                    probe.arch = Some(arch_from_output(&text).to_string());
                    probe.bits = Some(bits);
                }
                None => probe.error = Some("did not report a Java version".to_string()),
            }
        }
        Err(err) => probe.error = Some(err.to_string()),
    }

    probe
}

fn arch_from_output(text: &str) -> &'static str {
    if version::bits_from_output(text) == 64 {
        "x64"
    } else {
        "x86"
    }
}

fn file_stamp(path: &Path) -> (Option<i64>, Option<i64>) {
    let Ok(meta) = std::fs::metadata(path) else {
        return (None, None);
    };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);
    (mtime, Some(meta.len() as i64))
}

/// True when a cached entry still describes the binary on disk, so probing it
/// again would only burn a process spawn.
pub fn cache_is_fresh(cached: &CachedProbe, mtime: Option<i64>, size: Option<i64>) -> bool {
    cached.valid
        && cached.mtime.is_some()
        && cached.mtime == mtime
        && cached.size_bytes.is_some()
        && cached.size_bytes == size
        // Rows written before bitness was recorded have to be probed again:
        // treating an unknown width as usable is what let a 32-bit JVM be
        // picked in the first place.
        && cached.bits.is_some()
}

/// Probes every candidate, reusing cache entries whose binary is unchanged.
pub fn detect_all(cached: &[CachedProbe]) -> Vec<Probe> {
    let mut probes = Vec::new();
    for (binary, source) in candidates() {
        let path = binary.to_string_lossy().to_string();
        let (mtime, size_bytes) = file_stamp(&binary);

        if let Some(hit) = cached.iter().find(|c| c.path == path) {
            if cache_is_fresh(hit, mtime, size_bytes) {
                probes.push(Probe {
                    path,
                    source,
                    major: Some(hit.major),
                    full_version: hit.full_version.clone(),
                    vendor: hit.vendor.clone(),
                    arch: hit.arch.clone(),
                    bits: hit.bits,
                    mtime,
                    size_bytes,
                    error: None,
                });
                continue;
            }
        }

        probes.push(probe(&binary, source));
    }
    probes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cached(path: &str, mtime: Option<i64>, size: Option<i64>, valid: bool) -> CachedProbe {
        CachedProbe {
            path: path.to_string(),
            mtime,
            size_bytes: size,
            major: 21,
            full_version: Some("21.0.10".into()),
            vendor: Some("Temurin".into()),
            arch: Some("x64".into()),
            bits: Some(64),
            valid,
        }
    }

    #[test]
    fn a_cached_row_without_bitness_is_probed_again() {
        // Upgrading the app leaves rows detected before bitness existed. They
        // cannot be trusted for selection, so the stamp check is not enough.
        let mut entry = cached("/jdk/bin/java", Some(1000), Some(50), true);
        entry.bits = None;
        assert!(!cache_is_fresh(&entry, Some(1000), Some(50)));

        entry.bits = Some(32);
        assert!(cache_is_fresh(&entry, Some(1000), Some(50)), "known width is enough");
    }

    #[test]
    fn cache_hits_need_matching_mtime_and_size() {
        let entry = cached("/jdk/bin/java", Some(1000), Some(50), true);
        assert!(cache_is_fresh(&entry, Some(1000), Some(50)));
        assert!(!cache_is_fresh(&entry, Some(1001), Some(50)), "rebuilt binary");
        assert!(!cache_is_fresh(&entry, Some(1000), Some(51)), "different size");
        assert!(!cache_is_fresh(&entry, None, Some(50)), "unknown stamp");
    }

    #[test]
    fn invalid_cache_entries_are_always_reprobed() {
        let entry = cached("/jdk/bin/java", Some(1000), Some(50), false);
        assert!(!cache_is_fresh(&entry, Some(1000), Some(50)));
    }

    #[test]
    fn cache_entries_without_a_stamp_are_reprobed() {
        let entry = cached("/jdk/bin/java", None, None, true);
        assert!(!cache_is_fresh(&entry, Some(1000), Some(50)));
    }

    #[test]
    fn the_binary_name_follows_the_platform() {
        if cfg!(windows) {
            assert_eq!(java_executable_name(), "java.exe");
        } else {
            assert_eq!(java_executable_name(), "java");
        }
    }

    #[test]
    fn a_jdk_home_bin_folder_or_binary_all_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("jdk-21");
        let bin = home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let binary = bin.join(java_executable_name());
        std::fs::write(&binary, b"#!/bin/sh\n").unwrap();

        assert_eq!(java_binary_in(&home), Some(binary.clone()));
        assert_eq!(java_binary_in(&bin), Some(binary.clone()));
        assert_eq!(java_binary_in(&binary), Some(binary));
        assert_eq!(java_binary_in(dir.path()), None);
    }

    #[test]
    fn install_roots_are_platform_appropriate() {
        let roots = install_roots();
        assert!(!roots.is_empty());
        if cfg!(windows) {
            assert!(roots.iter().any(|r| r.ends_with("Eclipse Adoptium")));
        } else {
            assert!(roots.iter().any(|r| r == Path::new("/usr/lib/jvm")));
        }
    }

    #[test]
    fn probing_a_non_java_file_reports_an_error_rather_than_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("not-java.txt");
        std::fs::write(&fake, b"hello").unwrap();
        let result = probe(&fake, JavaSource::Manual);
        assert!(result.major.is_none());
        assert!(result.error.is_some());
    }

    /// This machine has JDKs installed, so detection must find at least one and
    /// every hit must carry a real major version read from the binary itself.
    #[test]
    fn detection_finds_real_runtimes_on_this_machine() {
        let probes = detect_all(&[]);
        let usable: Vec<&Probe> = probes.iter().filter(|p| p.major.is_some()).collect();
        assert!(!usable.is_empty(), "no Java found: {probes:?}");
        for probe in usable {
            assert!(probe.major.unwrap() >= 8);
            assert!(probe.full_version.is_some());
            assert!(probe.mtime.is_some());
        }
    }
}
