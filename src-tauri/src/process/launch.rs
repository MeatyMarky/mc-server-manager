//! Building the command line that starts a server.
//!
//! Three launch shapes exist, and they are not interchangeable:
//!   * `jar`       — `java <jvm args> -jar server.jar --nogui`
//!   * `args_file` — `java <jvm args> @libraries/…/unix_args.txt --nogui`
//!     (Forge/NeoForge ≥ 1.17: there is no runnable jar)
//!   * `script`    — the installer's own `run.sh` / `run.bat`
//!
//! The whole thing is a pure function over the instance row so it can be tested
//! for both platforms without spawning anything.

use std::path::{Path, PathBuf};

use crate::db::models::{Instance, LaunchKind};
use crate::error::{AppError, AppResult};

/// A ready-to-spawn command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub working_dir: PathBuf,
    /// Whether the program is a shell script rather than the JVM.
    pub is_script: bool,
}

/// Keeps a server from opening GUI windows — Swing error dialogs in particular.
pub const HEADLESS_FLAG: &str = "-Djava.awt.headless=true";

/// Heap flags derived from the instance's RAM settings.
pub fn heap_args(min_ram_mb: i64, max_ram_mb: i64) -> Vec<String> {
    let min = min_ram_mb.max(128);
    let max = max_ram_mb.max(min);
    vec![format!("-Xms{min}M"), format!("-Xmx{max}M")]
}

/// Renders one argument for a log line: quoted, with control characters and
/// trailing whitespace made visible.
///
/// A `-Xmx8192M\r` and a `-Xmx8192M` are the same width on screen and behave
/// very differently, so the log has to show the difference rather than imply it.
pub fn quote_arg(arg: &str) -> String {
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    for ch in arg.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 || ch as u32 == 0x7f => {
                out.push_str(&format!("\\u{{{:04x}}}", ch as u32))
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// The whole command line as one loggable string, every token quoted.
pub fn quoted_command(program: &Path, args: &[String]) -> String {
    let mut parts = vec![quote_arg(&program.to_string_lossy())];
    parts.extend(args.iter().map(|arg| quote_arg(arg)));
    parts.join(" ")
}

/// `-Xmx` / `-Xms` exactly as the JVM will accept them: the flag, digits, and an
/// optional single-letter unit. Nothing else — no trailing whitespace, no second
/// value glued on, no stray carriage return.
pub fn heap_flag_is_well_formed(arg: &str) -> bool {
    let Some(value) = arg
        .strip_prefix("-Xmx")
        .or_else(|| arg.strip_prefix("-Xms"))
    else {
        return true; // not a heap flag; nothing to say about it
    };

    let (digits, unit) = value.split_at(
        value
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(value.len()),
    );
    if digits.is_empty() || digits.parse::<u64>().is_err() {
        return false;
    }
    matches!(unit, "" | "k" | "K" | "m" | "M" | "g" | "G" | "t" | "T")
}

/// Rejects a command line the JVM would only complain about after spawning.
///
/// The JVM's own message names the flag but not where it came from, and a
/// malformed one is invisible in a console: `-Xmx8192M ` and `-Xmx8192M` look
/// identical. Checking here means the error can quote the exact token.
pub fn validate_args(args: &[String]) -> AppResult<()> {
    for arg in args {
        if !heap_flag_is_well_formed(arg) {
            return Err(AppError::Other(format!(
                "the memory setting {} is not a value the JVM accepts; \
                 check this server's JVM arguments",
                quote_arg(arg)
            )));
        }
    }

    // Two heap maxima on one command line is legal — the JVM takes the last —
    // but it means two settings disagree, and the one that wins is not the one
    // the user is looking at. Resolved in `plan`; this catches a regression.
    let maxima = args.iter().filter(|arg| arg.starts_with("-Xmx")).count();
    if maxima > 1 {
        return Err(AppError::Other(format!(
            "the launch command sets the maximum heap {maxima} times: {}",
            args.iter()
                .filter(|arg| arg.starts_with("-Xmx"))
                .map(|arg| quote_arg(arg))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    Ok(())
}

/// The heap this instance will really run with, in MB, resolved the same way
/// `plan` builds the command line.
///
/// `plan` writes `-Xms/-Xmx` from the RAM fields first and appends the custom
/// JVM arguments after them, so a custom `-Xmx` wins and the RAM field is what
/// applies when there is none. Anything checking the heap has to resolve it
/// through here rather than reading either source on its own.
pub fn effective_heap_mb(instance: &Instance) -> Option<i64> {
    let jvm_args: Vec<String> = decode_list(&instance.jvm_args);
    max_heap_bytes(instance.max_ram_mb, &jvm_args).map(|bytes| (bytes / (1024 * 1024)) as i64)
}

/// A script launch runs `run.bat`/`run.sh`, which reads its own JVM arguments
/// from `user_jvm_args.txt` next to it. The app's RAM fields do not apply, so
/// this is the only place a script's heap can come from.
pub fn script_heap_mb(instance_dir: &Path) -> Option<i64> {
    let text = std::fs::read_to_string(instance_dir.join("user_jvm_args.txt")).ok()?;
    args_file_tokens(&text)
        .iter()
        .filter_map(|arg| parse_xmx(arg))
        .next_back()
        .map(|bytes| (bytes / (1024 * 1024)) as i64)
}

/// Every argument in a JVM arguments file, cleaned.
///
/// These files ship with CRLF endings from the Forge and NeoForge installers and
/// are edited by hand on both platforms, so each token is trimmed of carriage
/// returns and surrounding whitespace. A `-Xmx8G\r` reaching a command line
/// fails with a message that shows no sign of the stray character.
pub fn args_file_tokens(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .flat_map(|line| line.split_whitespace())
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
        .collect()
}

/// The heap the JVM will actually be given, in bytes.
///
/// The instance's RAM setting is the default, but a custom `-Xmx` in the JVM
/// arguments is appended after it and so wins — the memory chart has to plot
/// against the limit the server really runs with, not the one the form shows.
pub fn max_heap_bytes(max_ram_mb: i64, jvm_args: &[String]) -> Option<u64> {
    let from_args = jvm_args
        .iter()
        .filter_map(|arg| parse_xmx(arg))
        .next_back();

    from_args.or_else(|| u64::try_from(max_ram_mb).ok().map(|mb| mb * 1024 * 1024))
}

/// `-Xmx4G`, `-Xmx4096m`, `-Xmx4096` (bytes), `-Xmx8g`.
fn parse_xmx(arg: &str) -> Option<u64> {
    let value = arg.strip_prefix("-Xmx").or_else(|| arg.strip_prefix("-XX:MaxHeapSize="))?;
    let (digits, unit) = value.split_at(value.find(|c: char| !c.is_ascii_digit()).unwrap_or(value.len()));
    let amount: u64 = digits.parse().ok()?;

    let scale = match unit.trim().to_ascii_lowercase().as_str() {
        "" => 1,
        "k" | "kb" => 1024,
        "m" | "mb" => 1024 * 1024,
        "g" | "gb" => 1024 * 1024 * 1024,
        "t" | "tb" => 1024_u64.pow(4),
        _ => return None,
    };
    amount.checked_mul(scale)
}

fn is_heap_flag(arg: &str) -> bool {
    arg.starts_with("-Xmx") || arg.starts_with("-Xms")
}

/// The heap flags for the command line, with the custom arguments folded in.
///
/// A custom `-Xmx`/`-Xms` replaces the generated one instead of being appended
/// after it, so the command line carries exactly one of each and it is the one
/// the user set.
pub fn heap_args_resolved(min_ram_mb: i64, max_ram_mb: i64, custom: &[String]) -> Vec<String> {
    let custom_min = custom.iter().rev().find(|arg| arg.starts_with("-Xms"));
    let custom_max = custom.iter().rev().find(|arg| arg.starts_with("-Xmx"));

    let generated = heap_args(min_ram_mb, max_ram_mb);
    vec![
        custom_min.cloned().unwrap_or_else(|| generated[0].clone()),
        custom_max.cloned().unwrap_or_else(|| generated[1].clone()),
    ]
}

fn decode_list(raw: &str) -> Vec<String> {
    serde_json::from_str(raw).unwrap_or_default()
}

/// Builds the launch plan. `java` is the resolved runtime; scripts ignore it and
/// use whatever the script itself picks up, which is why a pinned Java is
/// reported as ignored for script launches.
pub fn plan(instance: &Instance, java: &Path) -> AppResult<LaunchPlan> {
    let working_dir = instance.path_buf();
    let target = instance.launch_target.clone().unwrap_or_default();

    match instance.launch_kind {
        LaunchKind::Script => {
            if target.is_empty() {
                return Err(AppError::Other(format!(
                    "\"{}\" has no start script recorded; reinstall the server",
                    instance.name
                )));
            }
            let script = working_dir.join(&target);
            if !script.is_file() {
                return Err(AppError::Other(format!(
                    "the start script {} is missing; reinstall the server",
                    script.display()
                )));
            }
            // Windows runs .bat through the shell; Linux executes run.sh directly.
            let (program, args) = if cfg!(windows) {
                (
                    PathBuf::from("cmd"),
                    vec!["/C".to_string(), script.to_string_lossy().to_string()],
                )
            } else {
                (script, Vec::new())
            };
            Ok(LaunchPlan {
                program,
                args,
                working_dir,
                is_script: true,
            })
        }
        LaunchKind::Jar | LaunchKind::ArgsFile => {
            if target.is_empty() {
                return Err(AppError::Other(format!(
                    "\"{}\" has no server files installed yet",
                    instance.name
                )));
            }

            // The RAM fields and a custom -Xmx would otherwise both land on the
            // command line, leaving two settings where the winner is whichever
            // the JVM read last. One flag is emitted, and the custom one wins
            // because that is the existing behaviour people rely on.
            let custom = decode_list(&instance.jvm_args);
            let mut args = heap_args_resolved(instance.min_ram_mb, instance.max_ram_mb, &custom);
            // A server has no business opening a window. Fabric's launcher shows
            // its errors in a Swing dialog otherwise, which on a manager means a
            // stray window with the message this app should be showing.
            if !custom.iter().any(|arg| arg.starts_with("-Djava.awt.headless")) {
                args.push(HEADLESS_FLAG.to_string());
            }
            args.extend(custom.iter().filter(|arg| !is_heap_flag(arg)).cloned());

            if instance.launch_kind == LaunchKind::Jar {
                let jar = working_dir.join(&target);
                if !jar.is_file() {
                    return Err(AppError::Other(format!(
                        "the server jar {} is missing; reinstall the server",
                        jar.display()
                    )));
                }
                args.push("-jar".to_string());
                args.push(target.clone());
            } else {
                let args_file = working_dir.join(&target);
                if !args_file.is_file() {
                    return Err(AppError::Other(format!(
                        "the launch arguments file {} is missing; reinstall the server",
                        args_file.display()
                    )));
                }
                // Forge/NeoForge expect the @-file relative to the working dir.
                args.push(format!("@{target}"));
            }

            args.extend(decode_list(&instance.server_args));

            Ok(LaunchPlan {
                program: java.to_path_buf(),
                args,
                working_dir,
                is_script: false,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::ServerType;

    fn instance(kind: LaunchKind, target: &str, dir: &Path) -> Instance {
        Instance {
            id: 1,
            uuid: "u".into(),
            name: "Test".into(),
            path: dir.to_string_lossy().to_string(),
            server_type: ServerType::Paper,
            mc_version: "1.21.4".into(),
            loader_version: None,
            launch_kind: kind,
            launch_target: Some(target.to_string()),
            java_path: None,
            java_major: Some(21),
            jvm_args: r#"["-XX:+UseG1GC"]"#.into(),
            server_args: r#"["--nogui"]"#.into(),
            min_ram_mb: 1024,
            max_ram_mb: 4096,
            eula_accepted: true,
            eula_accepted_at: None,
            auto_start: false,
            auto_restart: false,
            restart_max: 3,
            restart_window_s: 600,
            stop_timeout_s: 60,
            rcon_enabled: false,
            rcon_port: None,
            rcon_password: None,
            color: None,
            notes: None,
            last_status: None,
            last_exit_code: None,
            last_started_at: None,
            last_stopped_at: None,
            pid: None,
            process_start_time: None,
            installed_artifact_url: None,
            installed_at: None,
            map_kind: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn heap_flags_respect_the_configured_range() {
        assert_eq!(heap_args(1024, 4096), vec!["-Xms1024M", "-Xmx4096M"]);
        // A max below the min is clamped rather than rejected by the JVM later.
        assert_eq!(heap_args(2048, 512), vec!["-Xms2048M", "-Xmx2048M"]);
    }

    #[test]
    fn a_jar_launch_puts_flags_before_jar_and_server_args_after() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("server.jar"), b"jar").unwrap();
        let plan = plan(
            &instance(LaunchKind::Jar, "server.jar", dir.path()),
            Path::new("/opt/java/bin/java"),
        )
        .unwrap();

        assert_eq!(plan.program, PathBuf::from("/opt/java/bin/java"));
        assert_eq!(
            plan.args,
            vec![
                "-Xms1024M",
                "-Xmx4096M",
                // Every server launch is headless: a manager must never leave a
                // Swing dialog on screen from a process it owns.
                HEADLESS_FLAG,
                "-XX:+UseG1GC",
                "-jar",
                "server.jar",
                "--nogui"
            ]
        );
        assert_eq!(plan.working_dir, dir.path());
        assert!(!plan.is_script);
    }

    #[test]
    fn an_args_file_launch_uses_the_at_syntax() {
        let dir = tempfile::tempdir().unwrap();
        let relative: PathBuf = ["libraries", "net", "neoforged", "neoforge", "21.4.157"]
            .iter()
            .collect();
        std::fs::create_dir_all(dir.path().join(&relative)).unwrap();
        let target = relative.join("unix_args.txt");
        std::fs::write(dir.path().join(&target), b"-p libraries").unwrap();

        let target = target.to_string_lossy().to_string();
        let plan = plan(
            &instance(LaunchKind::ArgsFile, &target, dir.path()),
            Path::new("/opt/java/bin/java"),
        )
        .unwrap();

        assert!(plan.args.contains(&format!("@{target}")));
        assert!(!plan.args.iter().any(|arg| arg == "-jar"));
        assert_eq!(plan.args.last().unwrap(), "--nogui");
    }

    #[test]
    fn a_script_launch_runs_the_script_itself() {
        let dir = tempfile::tempdir().unwrap();
        let script = if cfg!(windows) { "run.bat" } else { "run.sh" };
        std::fs::write(dir.path().join(script), b"echo hi").unwrap();

        let plan = plan(
            &instance(LaunchKind::Script, script, dir.path()),
            Path::new("/opt/java/bin/java"),
        )
        .unwrap();

        assert!(plan.is_script);
        if cfg!(windows) {
            assert_eq!(plan.program, PathBuf::from("cmd"));
            assert_eq!(plan.args.first().map(String::as_str), Some("/C"));
        } else {
            assert_eq!(plan.program, dir.path().join(script));
            assert!(plan.args.is_empty());
        }
    }

    #[test]
    fn missing_server_files_produce_a_readable_error_not_a_spawn_failure() {
        let dir = tempfile::tempdir().unwrap();
        let err = plan(
            &instance(LaunchKind::Jar, "server.jar", dir.path()),
            Path::new("java"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("reinstall the server"), "{err}");

        let err = plan(
            &instance(LaunchKind::ArgsFile, "libraries/x/args.txt", dir.path()),
            Path::new("java"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("launch arguments file"), "{err}");
    }

    #[test]
    fn an_instance_with_nothing_installed_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let mut inst = instance(LaunchKind::Jar, "", dir.path());
        inst.launch_target = None;
        let err = plan(&inst, Path::new("java")).unwrap_err();
        assert!(err.to_string().contains("no server files installed"), "{err}");
    }

    #[test]
    fn malformed_argument_json_does_not_break_the_launch() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("server.jar"), b"jar").unwrap();
        let mut inst = instance(LaunchKind::Jar, "server.jar", dir.path());
        inst.jvm_args = "not json".into();
        inst.server_args = "also not json".into();

        let plan = plan(&inst, Path::new("java")).unwrap();
        assert_eq!(
            plan.args,
            vec!["-Xms1024M", "-Xmx4096M", HEADLESS_FLAG, "-jar", "server.jar"]
        );
    }

    #[test]
    fn the_heap_limit_comes_from_the_ram_setting_unless_an_arg_overrides_it() {
        assert_eq!(max_heap_bytes(4096, &[]), Some(4096 * 1024 * 1024));

        // A custom -Xmx is appended after the generated one, so the JVM uses it
        // and so does the chart.
        let custom = vec!["-XX:+UseG1GC".to_string(), "-Xmx8G".to_string()];
        assert_eq!(max_heap_bytes(4096, &custom), Some(8 * 1024 * 1024 * 1024));

        // The last one wins, the same way the JVM reads them.
        let twice = vec!["-Xmx2G".to_string(), "-Xmx6G".to_string()];
        assert_eq!(max_heap_bytes(4096, &twice), Some(6 * 1024 * 1024 * 1024));
    }

    #[test]
    fn heap_sizes_parse_in_every_shape_the_jvm_accepts() {
        assert_eq!(parse_xmx("-Xmx1024"), Some(1024));
        assert_eq!(parse_xmx("-Xmx512k"), Some(512 * 1024));
        assert_eq!(parse_xmx("-Xmx2048M"), Some(2048 * 1024 * 1024));
        assert_eq!(parse_xmx("-Xmx3g"), Some(3 * 1024 * 1024 * 1024));
        assert_eq!(parse_xmx("-XX:MaxHeapSize=1G"), Some(1024 * 1024 * 1024));

        assert_eq!(parse_xmx("-Xms1G"), None, "the minimum is not the maximum");
        assert_eq!(parse_xmx("-XX:+UseG1GC"), None);
        assert_eq!(parse_xmx("-Xmxlots"), None);
    }

    #[test]
    fn the_effective_heap_is_the_ram_field_when_nothing_overrides_it() {
        // The shape that got past the first version of the guard: 8192 in the
        // RAM field and not an -Xmx in sight.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("server.jar"), b"jar").unwrap();
        let mut instance = instance(LaunchKind::Jar, "server.jar", dir.path());
        instance.max_ram_mb = 8192;
        instance.jvm_args = r#"["-XX:+UseG1GC","-XX:+ParallelRefProcEnabled"]"#.into();

        assert_eq!(effective_heap_mb(&instance), Some(8192));

        // And it is the same number the command line carries, which is what
        // makes checking it meaningful.
        let plan = plan(&instance, Path::new("/jdk/bin/java")).unwrap();
        assert!(
            plan.args.contains(&"-Xmx8192M".to_string()),
            "{:?}",
            plan.args
        );
    }

    #[test]
    fn a_custom_xmx_replaces_the_ram_field_rather_than_joining_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("server.jar"), b"jar").unwrap();
        let mut instance = instance(LaunchKind::Jar, "server.jar", dir.path());
        instance.max_ram_mb = 1024;
        instance.jvm_args = r#"["-Xmx6G"]"#.into();

        assert_eq!(effective_heap_mb(&instance), Some(6144));

        let plan = plan(&instance, Path::new("/jdk/bin/java")).unwrap();
        let maxima: Vec<&String> = plan
            .args
            .iter()
            .filter(|arg| arg.starts_with("-Xmx"))
            .collect();
        assert_eq!(
            maxima,
            vec!["-Xmx6G"],
            "one maximum on the command line, and it is the one the user set: {:?}",
            plan.args
        );
        assert!(validate_args(&plan.args).is_ok());
    }

    #[test]
    fn a_custom_xms_replaces_the_generated_one_too() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("server.jar"), b"jar").unwrap();
        let mut instance = instance(LaunchKind::Jar, "server.jar", dir.path());
        instance.jvm_args = r#"["-Xms512M","-XX:+UseG1GC"]"#.into();

        let plan = plan(&instance, Path::new("/jdk/bin/java")).unwrap();
        let minima: Vec<&String> = plan
            .args
            .iter()
            .filter(|arg| arg.starts_with("-Xms"))
            .collect();
        assert_eq!(minima, vec!["-Xms512M"], "{:?}", plan.args);
        // Everything that is not a heap flag survives untouched.
        assert!(plan.args.contains(&"-XX:+UseG1GC".to_string()));
    }

    #[test]
    fn an_args_file_with_crlf_endings_produces_clean_tokens() {
        // What the Forge and NeoForge installers write on Windows.
        let text = "# Xmx and Xms set the maximum and minimum RAM usage\r\n\r\n-Xmx8G\r\n-XX:+UseG1GC\r\n";
        let tokens = args_file_tokens(text);

        assert_eq!(tokens, vec!["-Xmx8G", "-XX:+UseG1GC"]);
        for token in &tokens {
            assert!(!token.contains('\r'), "{token:?} still carries a carriage return");
            assert_eq!(token.trim(), token, "{token:?} has stray whitespace");
            assert!(heap_flag_is_well_formed(token), "{token:?}");
        }

        // And the heap read out of such a file is the plain number.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("user_jvm_args.txt"), text.as_bytes()).unwrap();
        assert_eq!(script_heap_mb(dir.path()), Some(8192));
    }

    #[test]
    fn a_heap_flag_is_checked_against_what_the_jvm_accepts() {
        assert!(heap_flag_is_well_formed("-Xmx8192M"));
        assert!(heap_flag_is_well_formed("-Xms1024M"));
        assert!(heap_flag_is_well_formed("-Xmx8G"));
        assert!(heap_flag_is_well_formed("-Xmx8192"));
        assert!(heap_flag_is_well_formed("-XX:+UseG1GC"), "not a heap flag");

        // The shapes that would reach the JVM invisibly.
        assert!(!heap_flag_is_well_formed("-Xmx8192M\r"));
        assert!(!heap_flag_is_well_formed("-Xmx8192M "));
        assert!(!heap_flag_is_well_formed("-Xmx8192M8192M"));
        assert!(!heap_flag_is_well_formed("-Xmx8192MB"));
        assert!(!heap_flag_is_well_formed("-Xmx"));
        assert!(!heap_flag_is_well_formed("-XmxG"));
    }

    #[test]
    fn a_malformed_or_doubled_heap_flag_is_refused_before_spawning() {
        let err = validate_args(&["-Xmx8192M\r".to_string()]).unwrap_err();
        // The escape makes the stray character visible in the message itself.
        assert!(err.to_string().contains("\\r"), "{err}");

        let err = validate_args(&[
            "-Xmx1024M".to_string(),
            "-Xmx8G".to_string(),
        ])
        .unwrap_err();
        assert!(err.to_string().contains("2 times"), "{err}");

        assert!(validate_args(&["-Xms1024M".into(), "-Xmx4096M".into()]).is_ok());
    }

    #[test]
    fn arguments_are_logged_so_whitespace_and_control_characters_show() {
        assert_eq!(quote_arg("-Xmx8192M"), "\"-Xmx8192M\"");
        assert_eq!(quote_arg("-Xmx8192M\r"), "\"-Xmx8192M\\r\"");
        assert_eq!(quote_arg("-Xmx8192M "), "\"-Xmx8192M \"");
        assert_eq!(quote_arg("a\tb"), "\"a\\tb\"");
        assert_eq!(quote_arg("say \"hi\""), "\"say \\\"hi\\\"\"");

        let line = quoted_command(
            Path::new("C:/Program Files/Java/jdk-26/bin/java.exe"),
            &["-Xmx8192M".to_string(), "-jar".to_string(), "server.jar".to_string()],
        );
        assert_eq!(
            line,
            "\"C:/Program Files/Java/jdk-26/bin/java.exe\" \"-Xmx8192M\" \"-jar\" \"server.jar\""
        );
    }

    #[test]
    fn a_script_takes_its_heap_from_user_jvm_args_not_from_the_ram_fields() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(script_heap_mb(dir.path()), None, "no file, nothing to say");

        std::fs::write(
            dir.path().join("user_jvm_args.txt"),
            b"# Xmx and Xms set the maximum and minimum RAM usage\n-Xmx8G\n-XX:+UseG1GC\n",
        )
        .unwrap();
        assert_eq!(script_heap_mb(dir.path()), Some(8192));

        // Commented-out settings are not settings.
        std::fs::write(dir.path().join("user_jvm_args.txt"), b"#-Xmx8G\n").unwrap();
        assert_eq!(script_heap_mb(dir.path()), None);

        // Several on one line, last wins, same as the JVM.
        std::fs::write(dir.path().join("user_jvm_args.txt"), b"-Xmx2G -Xmx4G\n").unwrap();
        assert_eq!(script_heap_mb(dir.path()), Some(4096));
    }

    #[test]
    fn every_jar_launch_is_headless_and_a_custom_setting_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("server.jar"), b"jar").unwrap();

        let default_plan = plan(
            &instance(LaunchKind::Jar, "server.jar", dir.path()),
            Path::new("/opt/java/bin/java"),
        )
        .unwrap();
        assert_eq!(
            default_plan
                .args
                .iter()
                .filter(|arg| arg.starts_with("-Djava.awt.headless"))
                .count(),
            1
        );

        // Somebody who set it themselves — to anything — keeps their value.
        let mut chosen = instance(LaunchKind::Jar, "server.jar", dir.path());
        chosen.jvm_args = r#"["-Djava.awt.headless=false"]"#.into();
        let chosen_plan = plan(&chosen, Path::new("/opt/java/bin/java")).unwrap();
        let headless: Vec<&String> = chosen_plan
            .args
            .iter()
            .filter(|arg| arg.starts_with("-Djava.awt.headless"))
            .collect();
        assert_eq!(
            headless,
            vec!["-Djava.awt.headless=false"],
            "{:?}",
            chosen_plan.args
        );
    }
}
