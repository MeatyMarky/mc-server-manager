//! "Report a problem": the bundle a user attaches to an issue.
//!
//! Two rules shape this module. **Nothing leaves the machine on its own** — the
//! app writes a file the user chooses to attach, and never uploads. And **the
//! user sees exactly what is in it first**: `preview` returns the real text of
//! every part, not a description of it, so "the log" is something they can read
//! before deciding.

use std::path::{Path, PathBuf};

use serde::Serialize;
use ts_rs::TS;

use crate::db::now_rfc3339;
use crate::error::{AppError, AppResult, IoContext};
use crate::state::AppState;

/// Console and log lines included by default.
pub const DEFAULT_LINES: usize = 500;
/// Nothing bigger than this is copied in whole; the tail is taken instead.
const MAX_PART_BYTES: u64 = 2 * 1024 * 1024;

/// One file that would go into the zip.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct ReportPart {
    /// Name inside the zip.
    pub name: String,
    /// One line saying why it is useful, shown next to the preview.
    pub purpose: String,
    /// The exact text that will be written.
    pub content: String,
}

/// Everything the report will contain, before it is written anywhere.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct ReportPreview {
    pub parts: Vec<ReportPart>,
    /// Suggested file name for the zip.
    pub suggested_name: String,
    #[ts(type = "number")]
    pub total_bytes: i64,
    /// What a reader of the bundle can learn about this machine. Shown as a
    /// warning, because folder names carry the user's account name.
    pub notice: String,
}

/// Build information, also shown in the About dialog.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct BuildInfo {
    pub version: String,
    /// Commit the binary was built from, `unknown` outside a git checkout.
    pub git_sha: String,
    pub platform: String,
    pub arch: String,
    pub db_path: String,
    pub log_dir: String,
    pub instance_root: String,
    /// Highest applied migration, which is what a schema question really asks.
    #[ts(type = "number | null")]
    pub schema_version: Option<i64>,
}

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn git_sha() -> &'static str {
    env!("MSM_GIT_SHA")
}

pub fn log_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("logs")
}

pub fn db_path(data_dir: &Path) -> PathBuf {
    data_dir.join("msm.sqlite")
}

/// The migration version the database is actually on.
pub async fn schema_version(state: &AppState) -> Option<i64> {
    sqlx::query_scalar::<_, i64>("SELECT MAX(version) FROM _sqlx_migrations")
        .fetch_one(&state.db)
        .await
        .ok()
}

pub async fn build_info(state: &AppState) -> AppResult<BuildInfo> {
    let instance_root = crate::db::setting_get(&state.db, "default_instance_root")
        .await?
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| state.data_dir.join("instances").to_string_lossy().to_string());

    Ok(BuildInfo {
        version: version().to_string(),
        git_sha: git_sha().to_string(),
        platform: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        db_path: db_path(&state.data_dir).to_string_lossy().to_string(),
        log_dir: log_dir(&state.data_dir).to_string_lossy().to_string(),
        instance_root,
        schema_version: schema_version(state).await,
    })
}

/// Whether this machine can run a Minecraft server yet.
///
/// The first-run screen asks this before offering anything: telling somebody to
/// create a server and only mentioning at launch that there is no Java is the
/// kind of thing that makes an app feel broken.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct Readiness {
    /// True while detection has never been run on this machine.
    pub java_scan_pending: bool,
    #[ts(type = "number")]
    pub java_count: i64,
    /// Highest usable major version found.
    #[ts(type = "number | null")]
    pub newest_java: Option<i64>,
    /// Java needed by the newest Minecraft this app would install today.
    #[ts(type = "number")]
    pub recommended_java: i64,
    /// Set when nothing installed can run a current server.
    pub warning: Option<String>,
    #[ts(type = "number")]
    pub instance_count: i64,
}

/// The Java a current Minecraft release needs. The per-version answer still
/// comes from Mojang at install time; this is only for the first-run warning.
pub const RECOMMENDED_JAVA: i64 = 21;

pub async fn readiness(state: &AppState) -> AppResult<Readiness> {
    let runtimes = crate::java::list(&state.db).await.unwrap_or_default();
    let newest = runtimes
        .iter()
        .filter(|runtime| runtime.valid)
        .map(|runtime| runtime.major)
        .max();

    let instance_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM instances")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);

    let scanned = crate::db::setting_get(&state.db, "java_scanned_at")
        .await?
        .is_some();

    let warning = match newest {
        Some(major) if major >= RECOMMENDED_JAVA => None,
        Some(major) => Some(format!(
            "Only Java {major} was found. Current Minecraft versions need Java {RECOMMENDED_JAVA} \
             or newer; older ones will still run."
        )),
        None if scanned => Some(
            "No Java was found on this computer. A server needs one — install a JDK such as \
             Temurin or Microsoft OpenJDK, then rescan."
                .into(),
        ),
        None => None,
    };

    Ok(Readiness {
        java_scan_pending: !scanned,
        java_count: runtimes.len() as i64,
        newest_java: newest,
        recommended_java: RECOMMENDED_JAVA,
        warning,
        instance_count,
    })
}

/// One thing that was checked, and what it found.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct HealthCheck {
    /// Short label, e.g. "Database integrity".
    pub name: String,
    pub status: HealthStatus,
    /// One line saying what was found, whether it passed or not.
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub enum HealthStatus {
    Ok,
    /// Working, but worth knowing about.
    Warn,
    /// Something is broken and will be noticed sooner or later.
    Fail,
}

/// Everything the self-check looked at.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct Health {
    pub checks: Vec<HealthCheck>,
    /// The worst status among the checks, so the UI has one thing to show.
    pub status: HealthStatus,
    pub checked_at: String,
}

/// Runs the whole self-check.
///
/// Everything here is a question somebody ends up asking when the app behaves
/// oddly: is the schema current, is the file sound, is the Java this app
/// downloaded still there and still runnable, are the server folders where they
/// were left. One place to look, and it travels in the problem report.
pub async fn health(state: &AppState) -> Health {
    let mut checks = Vec::new();

    // Schema and migrations.
    let applied: Vec<(i64, String, bool)> = sqlx::query_as(
        "SELECT version, description, success FROM _sqlx_migrations ORDER BY version",
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    let failed: Vec<String> = applied
        .iter()
        .filter(|(_, _, success)| !success)
        .map(|(version, description, _)| format!("{version} {description}"))
        .collect();

    checks.push(match (applied.last(), failed.is_empty()) {
        (Some((version, _, _)), true) => HealthCheck {
            name: "Schema".into(),
            status: HealthStatus::Ok,
            detail: format!("version {version}, {} migrations applied", applied.len()),
        },
        (_, false) => HealthCheck {
            name: "Schema".into(),
            status: HealthStatus::Fail,
            detail: format!("migrations that did not finish: {}", failed.join(", ")),
        },
        (None, _) => HealthCheck {
            name: "Schema".into(),
            status: HealthStatus::Fail,
            detail: "no migrations are recorded at all".into(),
        },
    });

    // The file itself.
    checks.push(match crate::db::integrity_problems(&state.db).await {
        Ok(problems) if problems.is_empty() => {
            let free = crate::db::free_pages(&state.db).await.unwrap_or(0);
            HealthCheck {
                name: "Database integrity".into(),
                status: HealthStatus::Ok,
                detail: format!(
                    "sound, {} free page(s), auto-vacuum {}",
                    free,
                    match crate::db::auto_vacuum_mode(&state.db).await {
                        2 => "incremental",
                        1 => "full",
                        _ => "off",
                    }
                ),
            }
        }
        Ok(problems) => HealthCheck {
            name: "Database integrity".into(),
            status: HealthStatus::Fail,
            detail: format!("{} problem(s): {}", problems.len(), problems.join("; ")),
        },
        Err(err) => HealthCheck {
            name: "Database integrity".into(),
            status: HealthStatus::Warn,
            detail: format!("could not be checked: {}", err.user_message()),
        },
    });

    // Managed runtimes: present on disk, and still able to answer for themselves.
    let managed = crate::java::managed::list(state).await.unwrap_or_default();
    if managed.is_empty() {
        checks.push(HealthCheck {
            name: "Downloaded Java".into(),
            status: HealthStatus::Ok,
            detail: "none installed".into(),
        });
    } else {
        let mut broken = Vec::new();
        for runtime in &managed {
            let path = std::path::Path::new(&runtime.java_path);
            if !path.is_file() {
                broken.push(format!("Java {} is missing from disk", runtime.feature_version));
                continue;
            }
            match crate::java::probe_major(path).await {
                Some(major) if crate::java::satisfies(major, runtime.feature_version) => {}
                Some(major) => broken.push(format!(
                    "Java {} reports version {major}",
                    runtime.feature_version
                )),
                None => broken.push(format!(
                    "Java {} did not answer -version",
                    runtime.feature_version
                )),
            }
        }

        checks.push(if broken.is_empty() {
            HealthCheck {
                name: "Downloaded Java".into(),
                status: HealthStatus::Ok,
                detail: format!(
                    "{} runtime(s), all runnable: {}",
                    managed.len(),
                    managed
                        .iter()
                        .map(|runtime| format!("Java {}", runtime.feature_version))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }
        } else {
            HealthCheck {
                name: "Downloaded Java".into(),
                status: HealthStatus::Fail,
                detail: broken.join("; "),
            }
        });
    }

    // Instance folders. A missing one is recoverable, so it is a warning.
    let instances: Vec<(String, String)> = sqlx::query_as("SELECT name, path FROM instances")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    let missing: Vec<String> = instances
        .iter()
        .filter(|(_, path)| !std::path::Path::new(path).is_dir())
        .map(|(name, _)| name.clone())
        .collect();

    checks.push(if instances.is_empty() {
        HealthCheck {
            name: "Server folders".into(),
            status: HealthStatus::Ok,
            detail: "no servers yet".into(),
        }
    } else if missing.is_empty() {
        HealthCheck {
            name: "Server folders".into(),
            status: HealthStatus::Ok,
            detail: format!("{} server(s), every folder reachable", instances.len()),
        }
    } else {
        HealthCheck {
            name: "Server folders".into(),
            status: HealthStatus::Warn,
            detail: format!(
                "{} of {} not reachable: {} — use \"Locate folder…\" if they moved",
                missing.len(),
                instances.len(),
                missing.join(", ")
            ),
        }
    });

    let status = if checks.iter().any(|check| check.status == HealthStatus::Fail) {
        HealthStatus::Fail
    } else if checks.iter().any(|check| check.status == HealthStatus::Warn) {
        HealthStatus::Warn
    } else {
        HealthStatus::Ok
    };

    Health {
        checks,
        status,
        checked_at: now_rfc3339(),
    }
}

/// The newest rolled log file, which is the one that has today's run in it.
pub fn newest_log(data_dir: &Path) -> Option<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(log_dir(data_dir))
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect();

    // The appender names files `msm.log.YYYY-MM-DD`, so the name sorts by date.
    files.sort();
    files.pop()
}

/// The last `lines` lines of a file, or the whole thing when it is short.
pub fn tail_of(path: &Path, lines: usize) -> String {
    let Ok(metadata) = std::fs::metadata(path) else {
        return String::new();
    };

    let text = if metadata.len() > MAX_PART_BYTES {
        // Reading a 200 MB log into memory to show 500 lines would be silly.
        read_tail_bytes(path, MAX_PART_BYTES)
    } else {
        std::fs::read_to_string(path).unwrap_or_default()
    };

    let collected: Vec<&str> = text.lines().collect();
    let start = collected.len().saturating_sub(lines);
    collected[start..].join("\n")
}

fn read_tail_bytes(path: &Path, bytes: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};

    let Ok(mut file) = std::fs::File::open(path) else {
        return String::new();
    };
    let Ok(length) = file.metadata().map(|meta| meta.len()) else {
        return String::new();
    };
    if file.seek(SeekFrom::Start(length.saturating_sub(bytes))).is_err() {
        return String::new();
    }

    let mut buffer = Vec::new();
    let _ = file.read_to_end(&mut buffer);
    // The cut lands mid-character often enough to matter for non-ASCII logs.
    String::from_utf8_lossy(&buffer).into_owned()
}

/// Assembles the report. Nothing is written to disk here.
pub async fn preview(
    state: &AppState,
    instance_id: Option<i64>,
    lines: usize,
) -> AppResult<ReportPreview> {
    let lines = lines.clamp(20, 5_000);
    let info = build_info(state).await?;
    let mut parts = Vec::new();

    let integrity = crate::db::integrity_problems(&state.db)
        .await
        .unwrap_or_else(|err| vec![format!("the check itself failed: {err}")]);
    let health = health(state).await;

    parts.push(ReportPart {
        name: "about.txt".into(),
        purpose: "Which build this is, and where its files live.".into(),
        content: format!(
            "Minecraft Server Manager {}\ncommit: {}\nplatform: {} ({})\nschema version: {}\n\
             database: {}\nlogs: {}\ninstance root: {}\ndatabase health: {}\ngenerated: {}\n",
            info.version,
            info.git_sha,
            info.platform,
            info.arch,
            info.schema_version
                .map(|version| version.to_string())
                .unwrap_or_else(|| "unknown".into()),
            info.db_path,
            info.log_dir,
            info.instance_root,
            if integrity.is_empty() {
                "ok".to_string()
            } else {
                format!("{} problem(s): {}", integrity.len(), integrity.join("; "))
            },
            now_rfc3339(),
        ),
    });

    parts.push(ReportPart {
        name: "health.txt".into(),
        purpose: "The self-check: schema, database, downloaded Java, server folders.".into(),
        content: health
            .checks
            .iter()
            .map(|check| {
                format!(
                    "[{}] {}: {}",
                    match check.status {
                        HealthStatus::Ok => "ok",
                        HealthStatus::Warn => "warn",
                        HealthStatus::Fail => "FAIL",
                    },
                    check.name,
                    check.detail
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
    });

    let runtimes = crate::java::list(&state.db).await.unwrap_or_default();
    let java = if runtimes.is_empty() {
        "No Java runtimes detected.\n".to_string()
    } else {
        runtimes
            .iter()
            .map(|runtime| {
                format!(
                    "Java {} ({}, {}, {}) at {}{}",
                    runtime.major,
                    runtime.full_version.as_deref().unwrap_or("version unknown"),
                    runtime.vendor.as_deref().unwrap_or("vendor unknown"),
                    runtime.arch.as_deref().unwrap_or("arch unknown"),
                    runtime.path,
                    if runtime.valid { "" } else { " [did not answer -version]" }
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    parts.push(ReportPart {
        name: "java.txt".into(),
        purpose: "The Java runtimes this app can see. Most start-up problems are here.".into(),
        content: java,
    });

    if let Some(log) = newest_log(&state.data_dir) {
        parts.push(ReportPart {
            name: "app.log".into(),
            purpose: format!("The last {lines} lines the app wrote to its own log."),
            content: tail_of(&log, lines),
        });
    }

    if let Some(id) = instance_id {
        let row = crate::instance::get(&state.db, id).await?;
        parts.push(ReportPart {
            name: "instance.txt".into(),
            purpose: "How this server is set up: type, version, Java, memory, launch target."
                .into(),
            content: format!(
                "name: {}\ntype: {:?}\nminecraft: {}\nloader: {}\nlaunch: {:?} {}\n\
                 java: {}\nmemory: {} MB to {} MB\njvm args: {}\nstatus: {:?}\npath: {}\n",
                row.name,
                row.server_type,
                row.mc_version,
                row.loader_version.clone().unwrap_or_else(|| "-".into()),
                row.launch_kind,
                row.launch_target.clone().unwrap_or_else(|| "-".into()),
                row.java_path.clone().unwrap_or_else(|| "chosen automatically".into()),
                row.min_ram_mb,
                row.max_ram_mb,
                row.jvm_args,
                state.status_of(&row.uuid),
                row.path,
            ),
        });

        // The event history says what the app decided and when — restarts,
        // give-ups, failed starts, backups — which the console alone does not.
        let events: Vec<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT ts, kind, detail FROM instance_events
             WHERE instance_id = ? ORDER BY ts DESC LIMIT 50",
        )
        .bind(id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
        parts.push(ReportPart {
            name: "events.txt".into(),
            purpose: "What the app did with this server: starts, crashes, restarts, backups."
                .into(),
            content: if events.is_empty() {
                "Nothing recorded for this server yet.".into()
            } else {
                events
                    .into_iter()
                    .map(|(ts, kind, detail)| {
                        format!("{ts} {kind}: {}", detail.unwrap_or_default())
                    })
                    .collect::<Vec<_>>()
                    .join("
")
            },
        });

        let console: Vec<String> = state
            .supervisor
            .tail(&row.uuid, lines)
            .into_iter()
            .map(|line| line.raw)
            .collect();
        parts.push(ReportPart {
            name: "console.log".into(),
            purpose: format!("The last {lines} console lines from \"{}\".", row.name),
            content: if console.is_empty() {
                "This server has not printed anything since the app started.".into()
            } else {
                console.join("\n")
            },
        });
    }

    let total_bytes = parts.iter().map(|part| part.content.len() as i64).sum();

    Ok(ReportPreview {
        parts,
        suggested_name: format!(
            "msm-report-{}.zip",
            now_rfc3339().replace([':', '-'], "").replace('T', "-").trim_end_matches('Z')
        ),
        total_bytes,
        notice: "Folder paths include your user name, and the console can contain player names \
                 and chat. Read the parts above before attaching this to a public issue. Nothing \
                 is sent anywhere by this app — it only writes the file."
            .into(),
    })
}

/// Writes the previewed report to `target`.
pub fn write_zip(preview: &ReportPreview, target: &Path) -> AppResult<u64> {
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).ctx("create the folder for the report", parent)?;
    }

    let file = std::fs::File::create(target).ctx("write the report", target)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    for part in &preview.parts {
        zip.start_file(&part.name, options)
            .map_err(|e| AppError::internal("writing the report", e))?;
        zip.write_all(part.content.as_bytes())
            .ctx("write the report", target)?;
    }

    zip.finish()
        .map_err(|e| AppError::internal("writing the report", e))?;

    std::fs::metadata(target)
        .map(|meta| meta.len())
        .ctx("write the report", target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::InstanceStatus;

    async fn state_with_instance(dir: &Path) -> AppState {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let state = AppState::new(pool, dir.to_path_buf());
        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
                jvm_args, server_args, created_at, updated_at)
             VALUES ('u1', 'Survival', ?, 'paper', '1.21.4', 'jar', '[\"-XX:+UseG1GC\"]', '[]', ?, ?)",
        )
        .bind(dir.join("survival").to_string_lossy().to_string())
        .bind(&now)
        .bind(&now)
        .execute(&state.db)
        .await
        .unwrap();
        state
    }

    #[test]
    fn the_build_is_stamped_with_a_commit() {
        // "unknown" is the honest answer outside a checkout, and still a stamp.
        assert!(!git_sha().is_empty());
        assert!(!version().is_empty());
    }

    #[test]
    fn a_tail_takes_the_end_of_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("app.log");
        let body: String = (1..=100).map(|n| format!("line {n}\n")).collect();
        std::fs::write(&log, body).unwrap();

        let tail = tail_of(&log, 10);
        assert!(tail.starts_with("line 91"), "{tail}");
        assert!(tail.ends_with("line 100"), "{tail}");
        assert_eq!(tail.lines().count(), 10);

        // Asking for more than there is returns everything, not an error.
        assert_eq!(tail_of(&log, 10_000).lines().count(), 100);
        assert_eq!(tail_of(&dir.path().join("nope.log"), 10), "");
    }

    #[tokio::test]
    async fn the_preview_is_the_report_not_a_description_of_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(log_dir(dir.path())).unwrap();
        std::fs::write(
            log_dir(dir.path()).join("msm.log.2026-08-19"),
            b"INFO something happened\n",
        )
        .unwrap();

        let state = state_with_instance(dir.path()).await;
        state.set_status("u1", InstanceStatus::Stopped);

        let preview = preview(&state, Some(1), DEFAULT_LINES).await.unwrap();
        let names: Vec<&str> = preview.parts.iter().map(|part| part.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "about.txt",
                "health.txt",
                "java.txt",
                "app.log",
                "instance.txt",
                "events.txt",
                "console.log"
            ]
        );

        // Every part carries its real text, so the dialog can show it.
        let by_name = |name: &str| {
            preview
                .parts
                .iter()
                .find(|part| part.name == name)
                .unwrap_or_else(|| panic!("{name} is in the report"))
        };

        let about = &by_name("about.txt").content;
        assert!(about.contains(version()));
        let schema = schema_version(&state).await.expect("a migrated database");
        assert!(
            about.contains(&format!("schema version: {schema}")),
            "{about}"
        );
        assert!(by_name("app.log").content.contains("something happened"));
        assert!(by_name("instance.txt").content.contains("Survival"));
        // The self-check travels with it: schema, database, Java, folders.
        let health_text = &by_name("health.txt").content;
        assert!(health_text.contains("Schema"), "{health_text}");
        assert!(health_text.contains("Database integrity"), "{health_text}");
        assert!(health_text.contains("Downloaded Java"), "{health_text}");
        assert!(health_text.contains("Server folders"), "{health_text}");
        assert!(preview.parts.iter().all(|part| !part.purpose.is_empty()));

        // And the warning about what the paths give away is not optional.
        assert!(preview.notice.contains("user name"));
        assert!(preview.notice.contains("Nothing is sent anywhere"));
        assert!(preview.total_bytes > 0);
        assert!(preview.suggested_name.ends_with(".zip"));
    }

    #[tokio::test]
    async fn a_report_without_an_instance_still_has_the_app_parts() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_instance(dir.path()).await;

        let preview = preview(&state, None, DEFAULT_LINES).await.unwrap();
        let names: Vec<&str> = preview.parts.iter().map(|part| part.name.as_str()).collect();
        assert!(names.contains(&"about.txt"));
        assert!(names.contains(&"java.txt"));
        assert!(!names.contains(&"console.log"), "no server was selected");
    }

    #[tokio::test]
    async fn the_zip_holds_exactly_what_was_previewed() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_instance(dir.path()).await;
        let preview = preview(&state, Some(1), 50).await.unwrap();

        let target = dir.path().join("reports").join("msm-report.zip");
        let size = write_zip(&preview, &target).unwrap();
        assert!(size > 0);

        let file = std::fs::File::open(&target).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        assert_eq!(zip.len(), preview.parts.len());

        for part in &preview.parts {
            use std::io::Read;
            let mut entry = zip.by_name(&part.name).expect("part is in the zip");
            let mut body = String::new();
            entry.read_to_string(&mut body).unwrap();
            assert_eq!(body, part.content, "{} was changed on the way in", part.name);
        }
    }

    #[tokio::test]
    async fn the_line_count_is_clamped_rather_than_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_instance(dir.path()).await;

        let tiny = preview(&state, Some(1), 0).await.unwrap();
        assert!(tiny.parts.last().unwrap().purpose.contains("20"));

        let huge = preview(&state, Some(1), 10_000_000).await.unwrap();
        assert!(huge.parts.last().unwrap().purpose.contains("5000"));
    }

    #[tokio::test]
    async fn readiness_says_nothing_until_java_has_actually_been_looked_for() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_instance(dir.path()).await;

        let before = readiness(&state).await.unwrap();
        assert!(before.java_scan_pending, "detection has not run yet");
        assert_eq!(before.warning, None, "no warning while the answer is unknown");
        assert_eq!(before.instance_count, 1);
    }

    #[tokio::test]
    async fn a_machine_with_no_java_is_told_plainly_after_the_scan() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_instance(dir.path()).await;
        crate::db::setting_set(&state.db, "java_scanned_at", &now_rfc3339())
            .await
            .unwrap();

        let ready = readiness(&state).await.unwrap();
        assert!(!ready.java_scan_pending);
        assert_eq!(ready.java_count, 0);
        let warning = ready.warning.expect("a machine with no Java is warned");
        assert!(warning.contains("No Java was found"), "{warning}");
        assert!(warning.contains("Temurin"), "{warning}: name something to install");
    }

    #[tokio::test]
    async fn an_old_java_is_a_warning_and_not_a_refusal() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_instance(dir.path()).await;
        crate::db::setting_set(&state.db, "java_scanned_at", &now_rfc3339())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO java_runtimes (path, major, full_version, vendor, arch, source, valid,
                detected_at, mtime, size_bytes)
             VALUES ('/usr/bin/java', 17, '17.0.9', 'Temurin', 'x64', 'path', 1, ?, 0, 0)",
        )
        .bind(now_rfc3339())
        .execute(&state.db)
        .await
        .unwrap();

        let ready = readiness(&state).await.unwrap();
        assert_eq!(ready.newest_java, Some(17));
        let warning = ready.warning.expect("17 is short of the recommendation");
        assert!(warning.contains("Java 21"), "{warning}");
        assert!(warning.contains("will still run"), "{warning}: older servers work");
    }

    #[tokio::test]
    async fn a_current_java_needs_no_warning_at_all() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_instance(dir.path()).await;
        crate::db::setting_set(&state.db, "java_scanned_at", &now_rfc3339())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO java_runtimes (path, major, full_version, vendor, arch, source, valid,
                detected_at, mtime, size_bytes)
             VALUES ('/usr/bin/java', 25, '25.0.1', 'Temurin', 'x64', 'path', 1, ?, 0, 0)",
        )
        .bind(now_rfc3339())
        .execute(&state.db)
        .await
        .unwrap();

        let ready = readiness(&state).await.unwrap();
        assert_eq!(ready.warning, None);
        assert_eq!(ready.newest_java, Some(25));
    }

    #[tokio::test]
    async fn a_clean_install_passes_every_check() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(
            crate::db::connect_in_memory().await.unwrap(),
            dir.path().to_path_buf(),
        );

        let health = health(&state).await;
        assert_eq!(health.status, HealthStatus::Ok, "{:?}", health.checks);

        let names: Vec<&str> = health.checks.iter().map(|check| check.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Schema", "Database integrity", "Downloaded Java", "Server folders"]
        );

        // Each one says what it found, not merely that it looked.
        let schema = &health.checks[0];
        assert!(schema.detail.contains("migrations applied"), "{}", schema.detail);
        assert!(schema.detail.contains("version"), "{}", schema.detail);
        assert!(health.checks.iter().all(|check| !check.detail.is_empty()));
        assert!(!health.checked_at.is_empty());
    }

    #[tokio::test]
    async fn a_folder_that_moved_is_a_warning_not_a_failure() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_instance(dir.path()).await;

        let health = health(&state).await;
        // The instance's folder does not exist in this fixture.
        let folders = health
            .checks
            .iter()
            .find(|check| check.name == "Server folders")
            .unwrap();
        assert_eq!(folders.status, HealthStatus::Warn);
        assert!(folders.detail.contains("Survival"), "{}", folders.detail);
        assert!(folders.detail.contains("Locate folder"), "{}", folders.detail);

        // A recoverable state must not make the whole install look broken.
        assert_eq!(health.status, HealthStatus::Warn);
    }

    #[tokio::test]
    async fn a_managed_runtime_that_vanished_fails_the_check() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(
            crate::db::connect_in_memory().await.unwrap(),
            dir.path().to_path_buf(),
        );

        sqlx::query(
            "INSERT INTO managed_runtimes
                (feature_version, release_name, vendor, java_path, installed_at, size_bytes)
             VALUES (25, 'jdk-25.0.4+7', 'Eclipse Temurin', 'Z:/gone/bin/java', ?, 1)",
        )
        .bind(now_rfc3339())
        .execute(&state.db)
        .await
        .unwrap();

        let health = health(&state).await;
        let java = health
            .checks
            .iter()
            .find(|check| check.name == "Downloaded Java")
            .unwrap();
        assert_eq!(java.status, HealthStatus::Fail);
        assert!(java.detail.contains("missing from disk"), "{}", java.detail);
        assert_eq!(health.status, HealthStatus::Fail, "the worst status wins");
    }

    #[tokio::test]
    async fn the_database_check_reports_how_space_is_managed() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::connect_in_memory().await.unwrap();
        crate::db::enable_incremental_vacuum(&pool).await.unwrap();
        let state = AppState::new(pool, dir.path().to_path_buf());

        let health = health(&state).await;
        let db_check = health
            .checks
            .iter()
            .find(|check| check.name == "Database integrity")
            .unwrap();

        assert_eq!(db_check.status, HealthStatus::Ok);
        assert!(db_check.detail.contains("sound"), "{}", db_check.detail);
        assert!(
            db_check.detail.contains("auto-vacuum incremental"),
            "{}",
            db_check.detail
        );
    }
}
