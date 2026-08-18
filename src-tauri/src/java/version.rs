//! Parsing `java -version` output, and mapping a Minecraft version to the Java
//! it needs.
//!
//! The folder a JDK sits in is never trusted — `jdk-21` folders holding a
//! Java 17 do exist — so every candidate is resolved by running it and reading
//! what it says about itself.

use crate::mcversion;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaVersionInfo {
    /// 8, 17, 21, 25, …
    pub major: i64,
    /// The full version string as reported: "21.0.10", "1.8.0_402".
    pub full: String,
    /// "Eclipse Adoptium", "Oracle", "Zulu", … when the runtime line names one.
    pub vendor: Option<String>,
    /// True when the runtime is a JDK (server installers need one), false for a JRE.
    pub is_jdk: bool,
}

/// Parses the three-line block `java -version` writes to **stderr**:
///
/// ```text
/// openjdk version "21.0.10" 2026-01-20 LTS
/// OpenJDK Runtime Environment Temurin-21.0.10+7 (build 21.0.10+7-LTS)
/// OpenJDK 64-Bit Server VM Temurin-21.0.10+7 (build 21.0.10+7-LTS, mixed mode)
/// ```
pub fn parse_java_version(output: &str) -> Option<JavaVersionInfo> {
    let version_line = output
        .lines()
        .find(|line| line.contains(" version \""))?;
    let quoted = version_line.split('"').nth(1)?;
    let major = major_from_version_string(quoted)?;

    let vendor = output
        .lines()
        .find(|line| line.contains("Runtime Environment"))
        .and_then(parse_vendor);

    // A JRE announces itself as a runtime only; JDKs ship the compiler and say
    // "JDK" or carry a Server VM line. This is a hint, not a hard gate.
    let is_jdk = !output.to_ascii_lowercase().contains("jre");

    Some(JavaVersionInfo {
        major,
        full: quoted.to_string(),
        vendor,
        is_jdk,
    })
}

/// `"1.8.0_402"` -> 8, `"21.0.10"` -> 21, `"25-ea"` -> 25.
pub fn major_from_version_string(version: &str) -> Option<i64> {
    let cleaned: String = version
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let mut parts = cleaned.split('.').filter(|p| !p.is_empty());
    let first = parts.next()?.parse::<i64>().ok()?;
    if first == 1 {
        // Java 8 and older report 1.MAJOR.x
        parts.next()?.parse::<i64>().ok()
    } else {
        Some(first)
    }
}

fn parse_vendor(runtime_line: &str) -> Option<String> {
    for vendor in [
        "Temurin",
        "Adoptium",
        "Zulu",
        "Corretto",
        "GraalVM",
        "Microsoft",
        "Liberica",
        "SapMachine",
        "Oracle",
    ] {
        if runtime_line.contains(vendor) {
            return Some(vendor.to_string());
        }
    }
    None
}

/// The Java major version a Minecraft version needs, when Mojang's own metadata
/// is not at hand. Mojang publishes `javaVersion.majorVersion` per version and
/// that always wins; this table is the offline fallback.
pub fn required_java_for(mc_version: &str) -> i64 {
    // Calendar-era releases (26.x and later) ship on Java 25.
    if mcversion::parse(mc_version).is_some_and(|v| v.era == mcversion::Era::Calendar) {
        return 25;
    }
    if mcversion::at_least(mc_version, "1.20.5") {
        return 21;
    }
    if mcversion::at_least(mc_version, "1.17") {
        return 17;
    }
    8
}

/// Whether an installed runtime can run a server that requires `required`.
/// Newer Java runs older servers in every case that matters here; older Java
/// cannot run newer class files.
pub fn satisfies(installed_major: i64, required: i64) -> bool {
    installed_major >= required
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEMURIN_21: &str = r#"openjdk version "21.0.10" 2026-01-20 LTS
OpenJDK Runtime Environment Temurin-21.0.10+7 (build 21.0.10+7-LTS)
OpenJDK 64-Bit Server VM Temurin-21.0.10+7 (build 21.0.10+7-LTS, mixed mode, sharing)"#;

    const ORACLE_8: &str = r#"java version "1.8.0_402"
Java(TM) SE Runtime Environment (build 1.8.0_402-b06)
Java HotSpot(TM) 64-Bit Server VM (build 25.402-b06, mixed mode)"#;

    const OPENJDK_26: &str = r#"openjdk version "26.0.1" 2026-04-21
OpenJDK Runtime Environment (build 26.0.1+9-24)
OpenJDK 64-Bit Server VM (build 26.0.1+9-24, mixed mode, sharing)"#;

    #[test]
    fn parses_modern_output() {
        let info = parse_java_version(TEMURIN_21).unwrap();
        assert_eq!(info.major, 21);
        assert_eq!(info.full, "21.0.10");
        assert_eq!(info.vendor.as_deref(), Some("Temurin"));
        assert!(info.is_jdk);
    }

    #[test]
    fn parses_the_java_8_scheme() {
        let info = parse_java_version(ORACLE_8).unwrap();
        assert_eq!(info.major, 8, "1.8.0_402 is Java 8");
        assert_eq!(info.full, "1.8.0_402");
    }

    #[test]
    fn parses_vendorless_builds() {
        let info = parse_java_version(OPENJDK_26).unwrap();
        assert_eq!(info.major, 26);
        assert_eq!(info.vendor, None);
    }

    #[test]
    fn rejects_output_that_is_not_a_java_banner() {
        assert!(parse_java_version("").is_none());
        assert!(parse_java_version("bash: java: command not found").is_none());
        assert!(parse_java_version("Picked up JAVA_TOOL_OPTIONS: -Xmx1g").is_none());
    }

    #[test]
    fn ignores_noise_before_the_version_line() {
        let noisy = format!("Picked up JAVA_TOOL_OPTIONS: -Xmx1g\n{TEMURIN_21}");
        assert_eq!(parse_java_version(&noisy).unwrap().major, 21);
    }

    #[test]
    fn version_strings_reduce_to_a_major() {
        assert_eq!(major_from_version_string("21.0.10"), Some(21));
        assert_eq!(major_from_version_string("1.8.0_402"), Some(8));
        assert_eq!(major_from_version_string("25-ea"), Some(25));
        assert_eq!(major_from_version_string("nonsense"), None);
    }

    #[test]
    fn maps_minecraft_versions_to_java_requirements() {
        assert_eq!(required_java_for("1.12.2"), 8);
        assert_eq!(required_java_for("1.16.5"), 8);
        assert_eq!(required_java_for("1.17"), 17);
        assert_eq!(required_java_for("1.20.4"), 17);
        assert_eq!(required_java_for("1.20.5"), 21);
        assert_eq!(required_java_for("1.21.4"), 21);
        // The calendar era moved to Java 25.
        assert_eq!(required_java_for("26.1"), 25);
        assert_eq!(required_java_for("26.2"), 25);
    }

    #[test]
    fn newer_runtimes_satisfy_older_requirements() {
        assert!(satisfies(26, 21));
        assert!(satisfies(21, 21));
        assert!(!satisfies(17, 21));
        assert!(!satisfies(21, 25));
    }
}
