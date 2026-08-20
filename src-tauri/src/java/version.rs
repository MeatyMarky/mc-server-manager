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
    /// 64 or 32. A 32-bit JVM cannot address the heap a server wants and refuses
    /// to start with `-Xmx8192M`, so this decides whether it is offered at all.
    pub bits: i64,
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
        bits: bits_from_output(output),
    })
}

/// Heap size a 32-bit JVM can still be asked for, in MB.
///
/// The hard ceiling is a 4 GB address space minus everything else the process
/// maps, which in practice lands somewhere between 1.4 and 1.6 GB on Windows.
/// Past this the JVM refuses to start with "Invalid maximum heap size", so the
/// app stops before spawning it rather than showing that in the console.
pub const MAX_HEAP_32BIT_MB: i64 = 1500;

/// 64 unless the VM line says otherwise.
///
/// Every 64-bit JVM prints "64-Bit Server VM" (or "64-Bit Client VM"); 32-bit
/// builds print "Client VM" or "Server VM" with no width at all. The absence is
/// the signal, so anything that does not say 64-bit is treated as 32-bit —
/// wrongly excluding an exotic JVM is a smaller failure than launching one that
/// cannot hold the heap.
pub fn bits_from_output(output: &str) -> i64 {
    let lowered = output.to_ascii_lowercase();
    if lowered.contains("64-bit") || lowered.contains("64 bit") {
        64
    } else {
        32
    }
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

/// How strictly a server type wants the Java version its Minecraft release
/// was built against.
///
/// Vanilla and the Bukkit-family servers are happy on anything newer: they are
/// plain Java programs, and a 1.16 server on Java 17 behaves. The mod loaders
/// are not, because Mixin rewrites bytecode at load time and a class file
/// format it does not know about produces failures deep inside somebody else's
/// mod — the kind nobody can diagnose from the stack trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub enum JavaFit {
    /// At or above the requirement; the lowest suitable one is chosen.
    Floor,
    /// The major version this Minecraft release specifies, and no other.
    Exact,
}

impl JavaFit {
    /// Whether an installed major version fits under this rule.
    pub fn accepts(self, installed_major: i64, required: i64) -> bool {
        match self {
            JavaFit::Floor => satisfies(installed_major, required),
            JavaFit::Exact => installed_major == required,
        }
    }
}

/// The rule for a server type.
pub fn fit_for(server_type: crate::db::models::ServerType) -> JavaFit {
    use crate::db::models::ServerType;
    match server_type {
        ServerType::Vanilla | ServerType::Paper | ServerType::Purpur => JavaFit::Floor,
        ServerType::Fabric | ServerType::Forge | ServerType::NeoForge => JavaFit::Exact,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::ServerType;

    #[test]
    fn plain_servers_take_anything_newer_and_loaders_do_not() {
        assert_eq!(fit_for(ServerType::Vanilla), JavaFit::Floor);
        assert_eq!(fit_for(ServerType::Paper), JavaFit::Floor);
        assert_eq!(fit_for(ServerType::Purpur), JavaFit::Floor);

        // Mixin rewrites bytecode as it loads, and a class file format it does
        // not know about fails somewhere inside a mod rather than saying so.
        assert_eq!(fit_for(ServerType::Fabric), JavaFit::Exact);
        assert_eq!(fit_for(ServerType::Forge), JavaFit::Exact);
        assert_eq!(fit_for(ServerType::NeoForge), JavaFit::Exact);
    }

    #[test]
    fn a_floor_accepts_the_requirement_and_everything_above_it() {
        assert!(JavaFit::Floor.accepts(8, 8));
        assert!(JavaFit::Floor.accepts(17, 8));
        assert!(JavaFit::Floor.accepts(25, 8));
        assert!(!JavaFit::Floor.accepts(8, 17), "older is never enough");
    }

    #[test]
    fn an_exact_rule_accepts_only_that_major() {
        // The case this exists for: a 1.16.5 Forge server and a Java 17.
        assert!(JavaFit::Exact.accepts(8, 8));
        assert!(!JavaFit::Exact.accepts(17, 8));
        assert!(!JavaFit::Exact.accepts(21, 8));
        assert!(!JavaFit::Exact.accepts(8, 17));
    }

    /// The two rules disagree in exactly one direction: newer than required.
    /// Anything a loader accepts, a plain server accepts too.
    #[test]
    fn exact_is_never_looser_than_floor() {
        for required in [8, 17, 21, 25] {
            for installed in [8, 11, 17, 21, 25, 26] {
                if JavaFit::Exact.accepts(installed, required) {
                    assert!(
                        JavaFit::Floor.accepts(installed, required),
                        "Java {installed} against a requirement of {required}"
                    );
                }
            }
        }
    }


    const TEMURIN_21: &str = r#"openjdk version "21.0.10" 2026-01-20 LTS
OpenJDK Runtime Environment Temurin-21.0.10+7 (build 21.0.10+7-LTS)
OpenJDK 64-Bit Server VM Temurin-21.0.10+7 (build 21.0.10+7-LTS, mixed mode, sharing)"#;

    const ORACLE_8: &str = r#"java version "1.8.0_402"
Java(TM) SE Runtime Environment (build 1.8.0_402-b06)
Java HotSpot(TM) 64-Bit Server VM (build 25.402-b06, mixed mode)"#;

    const OPENJDK_26: &str = r#"openjdk version "26.0.1" 2026-04-21
OpenJDK Runtime Environment (build 26.0.1+9-24)
OpenJDK 64-Bit Server VM (build 26.0.1+9-24, mixed mode, sharing)"#;

    /// The 32-bit Java 8 that ships into `Program Files (x86)`: no width in the
    /// VM line at all. This is the runtime that produced "Invalid maximum heap
    /// size: -Xmx8192M" after being picked automatically.
    const ORACLE_8_X86: &str = r#"java version "1.8.0_451"
Java(TM) SE Runtime Environment (build 1.8.0_451-b10)
Java HotSpot(TM) Client VM (build 25.451-b10, mixed mode, sharing)"#;

    const ZULU_17_X86: &str = r#"openjdk version "17.0.9" 2023-10-17 LTS
OpenJDK Runtime Environment Zulu17.46+19-CA (build 17.0.9+8-LTS)
OpenJDK Server VM Zulu17.46+19-CA (build 17.0.9+8-LTS, mixed mode)"#;

    #[test]
    fn parses_modern_output() {
        let info = parse_java_version(TEMURIN_21).unwrap();
        assert_eq!(info.major, 21);
        assert_eq!(info.full, "21.0.10");
        assert_eq!(info.vendor.as_deref(), Some("Temurin"));
        assert!(info.is_jdk);
    }

    /// Oracle's two shim directories, recorded from this machine.
    ///
    /// `Common Files\\Oracle\\Java\\javapath\\java.exe` is 64-bit and
    /// `Common Files\\Oracle\\Java\\java8path\\java.exe` is 32-bit — different
    /// folder names, the same file name, and neither says which is which. The
    /// `(x86)` in one path is not a signal either: a 32-bit JRE installs a shim
    /// under the 64-bit Program Files as well.
    const ORACLE_SHIMS: [(&str, &str); 2] = [
        (
            r"C:\Program Files\Common Files\Oracle\Java\javapath\java.exe",
            include_str!("../../tests/fixtures/java_version_oracle_shim_x64.txt"),
        ),
        (
            r"C:\Program Files (x86)\Common Files\Oracle\Java\java8path\java.exe",
            include_str!("../../tests/fixtures/java_version_oracle_shim_x86.txt"),
        ),
    ];

    #[test]
    fn the_oracle_shims_are_told_apart_by_what_they_print_not_by_their_paths() {
        let [(path_64, output_64), (path_32, output_32)] = ORACLE_SHIMS;

        assert_eq!(parse_java_version(output_64).unwrap().bits, 64);
        assert_eq!(parse_java_version(output_32).unwrap().bits, 32);

        // Both binaries are called java.exe and both live under "Common
        // Files\Oracle\Java": nothing in the path distinguishes them.
        assert!(path_64.ends_with(r"\java.exe") && path_32.ends_with(r"\java.exe"));
        assert!(path_64.contains(r"Common Files\Oracle\Java"));
        assert!(path_32.contains(r"Common Files\Oracle\Java"));

        // The shortcut this test exists to prevent: guessing from the path.
        // "(x86)" happens to line up here and does not in general — a 32-bit JRE
        // also installs a shim under the 64-bit Program Files — so any code that
        // reads a width out of a path is wrong even when it looks right.
        for (path, output) in ORACLE_SHIMS {
            let from_output = parse_java_version(output).unwrap().bits;
            let from_path = if path.contains("(x86)") { 32 } else { 64 };
            assert_eq!(
                from_output, from_path,
                "this machine happens to agree — the point is that only {from_output} was read \
                 from the binary, and {from_path} was a guess"
            );
            assert_eq!(from_output, bits_from_output(output));
        }

        // And the versions differ too, which is the other half of why the shim
        // that PATH resolves to is not the one a user assumes.
        assert_eq!(parse_java_version(output_64).unwrap().major, 26);
        assert_eq!(parse_java_version(output_32).unwrap().major, 8);
    }

    #[test]
    fn bitness_comes_from_the_vm_line_and_its_absence_means_32() {
        // Every 64-bit build says so; nothing else does.
        assert_eq!(parse_java_version(TEMURIN_21).unwrap().bits, 64);
        assert_eq!(parse_java_version(ORACLE_8).unwrap().bits, 64);
        assert_eq!(parse_java_version(OPENJDK_26).unwrap().bits, 64);

        // A 32-bit JVM prints "Client VM" or "Server VM" with no width.
        assert_eq!(parse_java_version(ORACLE_8_X86).unwrap().bits, 32);
        assert_eq!(parse_java_version(ZULU_17_X86).unwrap().bits, 32);
    }

    #[test]
    fn the_bitness_marker_is_matched_whatever_the_casing() {
        assert_eq!(bits_from_output("OpenJDK 64-Bit Server VM"), 64);
        assert_eq!(bits_from_output("openjdk 64-bit server vm"), 64);
        // Some builds space it out.
        assert_eq!(bits_from_output("Java HotSpot(TM) 64 Bit Server VM"), 64);

        assert_eq!(bits_from_output("Java HotSpot(TM) Client VM"), 32);
        assert_eq!(bits_from_output(""), 32, "unknown is treated as 32-bit");
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
