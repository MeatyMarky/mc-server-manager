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

/// Heap flags derived from the instance's RAM settings.
pub fn heap_args(min_ram_mb: i64, max_ram_mb: i64) -> Vec<String> {
    let min = min_ram_mb.max(128);
    let max = max_ram_mb.max(min);
    vec![format!("-Xms{min}M"), format!("-Xmx{max}M")]
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

            let mut args = heap_args(instance.min_ram_mb, instance.max_ram_mb);
            args.extend(decode_list(&instance.jvm_args));

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
            vec!["-Xms1024M", "-Xmx4096M", "-XX:+UseG1GC", "-jar", "server.jar", "--nogui"]
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
        assert_eq!(plan.args, vec!["-Xms1024M", "-Xmx4096M", "-jar", "server.jar"]);
    }
}
