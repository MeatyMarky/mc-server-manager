//! Network smoke tests. Every one is `#[ignore]`, so CI stays offline and a bad
//! day at Mojang or PaperMC cannot break the build. Run them by hand when
//! touching providers, the downloader, or the installer:
//!
//! ```text
//! cargo test --test network_smoke -- --ignored --nocapture
//! ```

use std::path::{Path, PathBuf};

use mc_server_manager_lib::db;
use mc_server_manager_lib::db::models::{LaunchKind, ServerType};
use mc_server_manager_lib::download;
use mc_server_manager_lib::http::Http;
use mc_server_manager_lib::instance::{self, crud, install, CreateInstanceInput};
use mc_server_manager_lib::providers;
use mc_server_manager_lib::state::AppState;
use tokio_util::sync::CancellationToken;

async fn state_in(dir: &Path) -> AppState {
    let pool = db::connect_in_memory().await.expect("in-memory database");
    AppState::new(pool, dir.to_path_buf())
}

async fn instance_in(
    state: &AppState,
    dir: &Path,
    name: &str,
    server_type: ServerType,
    mc_version: &str,
) -> mc_server_manager_lib::db::models::Instance {
    crud::create(
        state,
        CreateInstanceInput {
            name: name.to_string(),
            path: dir.join(name).to_string_lossy().to_string(),
            server_type,
            mc_version: mc_version.to_string(),
            loader_version: None,
            min_ram_mb: None,
            max_ram_mb: None,
            notes: None,
            color: None,
        },
    )
    .await
    .expect("create instance")
}

/// Every provider resolves against the live APIs.
#[tokio::test]
#[ignore = "hits the network"]
async fn all_six_providers_resolve_live() {
    let http = Http::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let state = state_in(dir.path()).await;
    // Release chronology from the live manifest drives every version sort.
    let index = providers::index::refresh(&state.db, &http).await.unwrap();
    assert!(!index.is_empty());

    for (server_type, mc_version) in [
        (ServerType::Vanilla, "1.21.4"),
        (ServerType::Paper, "1.21.4"),
        (ServerType::Purpur, "1.21.4"),
        (ServerType::Fabric, "1.21.4"),
        (ServerType::Forge, "1.21.4"),
        (ServerType::NeoForge, "1.21.4"),
    ] {
        let versions = providers::list_versions(server_type, &http, &index)
            .await
            .unwrap_or_else(|e| panic!("{server_type:?} versions: {e}"));
        assert!(!versions.is_empty(), "{server_type:?} listed no versions");
        let ids: Vec<&str> = versions.iter().map(|v| v.id.as_str()).collect();
        assert!(
            ids.windows(2).all(|w| !index.is_newer(w[1], w[0])),
            "{server_type:?} versions are not in release order: {:?}",
            &ids[..ids.len().min(8)]
        );

        let artifact = providers::resolve(server_type, &http, mc_version, None)
            .await
            .unwrap_or_else(|e| panic!("{server_type:?} resolve: {e}"));
        assert!(artifact.url.starts_with("https://"), "{server_type:?}");
        println!(
            "{server_type:?} {mc_version} -> {} ({:?}, build {:?})",
            artifact.url, artifact.kind, artifact.build
        );
    }
}

/// A real download: verified checksum, `.part` gone, final file present.
#[tokio::test]
#[ignore = "downloads ~50 MB"]
async fn paper_jar_downloads_and_verifies() {
    let dir = tempfile::tempdir().unwrap();
    let http = Http::new().unwrap();
    let artifact = providers::resolve(ServerType::Paper, &http, "1.21.4", None)
        .await
        .unwrap();

    let target = dir.path().join(&artifact.file_name);
    let cancel = CancellationToken::new();
    let bytes = download::download(&http, &artifact, &target, &cancel, |p| {
        if p.downloaded % (8 * 1024 * 1024) < 300_000 {
            println!("  {} / {:?}", p.downloaded, p.total);
        }
    })
    .await
    .expect("download");

    assert!(bytes > 1_000_000);
    assert!(target.is_file(), "final file exists");
    assert!(!download::part_path(&target).exists(), ".part is renamed away");
    // Re-running reuses the verified file instead of downloading again.
    let again = download::download(&http, &artifact, &target, &cancel, |_| {})
        .await
        .unwrap();
    assert_eq!(again, 0, "verified cache hit");
}

/// Cancel mid-transfer, then resume: the `.part` survives the cancel and the
/// second attempt finishes the same file.
#[tokio::test]
#[ignore = "downloads ~50 MB"]
async fn a_cancelled_download_resumes() {
    let dir = tempfile::tempdir().unwrap();
    let http = Http::new().unwrap();
    let artifact = providers::resolve(ServerType::Paper, &http, "1.21.4", None)
        .await
        .unwrap();
    let target = dir.path().join(&artifact.file_name);

    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    let err = download::download(&http, &artifact, &target, &cancel, move |p| {
        if p.downloaded > 2 * 1024 * 1024 {
            trigger.cancel();
        }
    })
    .await
    .unwrap_err();
    assert_eq!(err.kind(), "cancelled");

    let part = download::part_path(&target);
    let partial_len = std::fs::metadata(&part).expect("partial kept").len();
    assert!(partial_len > 0, "partial bytes are kept for the retry");
    assert!(!target.exists(), "no final file from a cancelled transfer");

    let fresh = CancellationToken::new();
    let transferred = download::download(&http, &artifact, &target, &fresh, |_| {})
        .await
        .expect("resume");
    assert!(target.is_file());
    assert!(
        transferred >= partial_len,
        "resumed total {transferred} should include the {partial_len} already on disk"
    );
    println!("resumed after {partial_len} bytes, total {transferred}");
}

/// End-to-end vanilla install into a real instance folder.
#[tokio::test]
#[ignore = "downloads ~50 MB"]
async fn vanilla_installs_into_an_instance() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let state = state_in(&root).await;
    let inst = instance_in(&state, &root, "vanilla", ServerType::Vanilla, "1.21.4").await;

    let cancel = CancellationToken::new();
    let outcome = install::install(
        &state,
        &state.http,
        &inst,
        "1.21.4",
        None,
        &cancel,
        |phase, done, total, msg| println!("  {phase:?} {done}/{total:?} {msg}"),
    )
    .await
    .expect("install");

    assert_eq!(outcome.launch_kind, LaunchKind::Jar);
    assert_eq!(outcome.launch_target.as_deref(), Some("server.jar"));
    assert_eq!(outcome.java_major, 21, "Mojang states the Java requirement");
    assert!(inst.path_buf().join("server.jar").is_file());
    // The EULA is still untouched.
    assert!(!inst.path_buf().join("eula.txt").exists());
}

/// The riskiest path in phase 2: run the Forge installer headlessly and check
/// the instance ends up launchable.
#[tokio::test]
#[ignore = "downloads and runs the Forge installer"]
async fn forge_installer_produces_a_launchable_instance() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let state = state_in(&root).await;
    mc_server_manager_lib::java::rescan(&state.db)
        .await
        .expect("java rescan");

    let inst = instance_in(&state, &root, "forge", ServerType::Forge, "1.21.4").await;
    let cancel = CancellationToken::new();
    let outcome = install::install(
        &state,
        &state.http,
        &inst,
        "1.21.4",
        None,
        &cancel,
        |phase, done, total, msg| println!("  {phase:?} {done}/{total:?} {msg}"),
    )
    .await;

    match outcome {
        Ok(outcome) => {
            println!(
                "forge installed: {:?} -> {:?}",
                outcome.launch_kind, outcome.launch_target
            );
            assert!(matches!(
                outcome.launch_kind,
                LaunchKind::ArgsFile | LaunchKind::Script | LaunchKind::Jar
            ));
            assert!(inst.path_buf().join("libraries").is_dir());
            // Staging is always cleaned up.
            assert!(!inst.path_buf().join(".msm").join("staging").exists());
        }
        Err(err) => {
            // A failure must be the structured kind, with a log to show.
            assert_eq!(err.kind(), "installer_failed", "unexpected error: {err}");
            assert!(!inst.path_buf().join("libraries").exists(), "no half-written install");
            panic!("forge installer failed (this is the case worth investigating): {err}");
        }
    }
}

/// NeoForge, same shape as Forge but a different installer.
#[tokio::test]
#[ignore = "downloads and runs the NeoForge installer"]
async fn neoforge_installer_produces_a_launchable_instance() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let state = state_in(&root).await;
    mc_server_manager_lib::java::rescan(&state.db).await.unwrap();

    let inst = instance_in(&state, &root, "neoforge", ServerType::NeoForge, "1.21.4").await;
    let cancel = CancellationToken::new();
    let outcome = install::install(
        &state,
        &state.http,
        &inst,
        "1.21.4",
        None,
        &cancel,
        |phase, done, total, msg| println!("  {phase:?} {done}/{total:?} {msg}"),
    )
    .await
    .expect("neoforge install");

    println!(
        "neoforge installed: {:?} -> {:?}",
        outcome.launch_kind, outcome.launch_target
    );
    assert!(inst.path_buf().join("libraries").is_dir());
}

/// Java detection against the machine running the test.
#[tokio::test]
#[ignore = "depends on the local machine"]
async fn java_detection_reports_real_runtimes() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_in(dir.path()).await;
    let runtimes = mc_server_manager_lib::java::rescan(&state.db).await.unwrap();
    assert!(!runtimes.is_empty(), "no Java found");
    for runtime in &runtimes {
        println!(
            "Java {} {:?} {} ({:?})",
            runtime.major, runtime.vendor, runtime.path, runtime.source
        );
    }
    // A second scan must be a cache hit and produce the same set.
    let again = mc_server_manager_lib::java::rescan(&state.db).await.unwrap();
    assert_eq!(again.len(), runtimes.len());
}

/// Guard: `instance::get` still round-trips after an install writes to the row.
#[tokio::test]
#[ignore = "hits the network"]
async fn install_updates_the_instance_row() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let state = state_in(&root).await;
    let inst = instance_in(&state, &root, "fabric", ServerType::Fabric, "1.21.4").await;

    let cancel = CancellationToken::new();
    install::install(
        &state,
        &state.http,
        &inst,
        "1.21.4",
        None,
        &cancel,
        |_, _, _, _| {},
    )
    .await
    .expect("fabric install");

    let reloaded = instance::get(&state.db, inst.id).await.unwrap();
    assert_eq!(reloaded.mc_version, "1.21.4");
}

/// A real Paper server, started with the launch plan this app builds: it must
/// reach "Done", answer a command on stdin, and stop cleanly when told to.
///
/// The supervisor itself needs a Tauri `AppHandle` to emit events, so this test
/// drives the same launch plan directly and checks the behaviour the supervisor
/// depends on: the argv is right, the output parses, stdin is accepted, and
/// `stop` ends the process.
#[tokio::test]
#[ignore = "downloads Paper and runs a real server"]
async fn a_real_server_starts_answers_and_stops() {
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    use mc_server_manager_lib::logparse::{self, LogEvent};
    use mc_server_manager_lib::process::console::ConsoleBuffer;
    use mc_server_manager_lib::process::launch;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let state = state_in(&root).await;
    mc_server_manager_lib::java::rescan(&state.db).await.unwrap();

    let inst = instance_in(&state, &root, "paper", ServerType::Paper, "1.21.4").await;
    let cancel = CancellationToken::new();
    install::install(&state, &state.http, &inst, "1.21.4", None, &cancel, |_, _, _, _| {})
        .await
        .expect("install paper");

    // The EULA is written only through the explicit acceptance path.
    mc_server_manager_lib::instance::eula::set(&state, inst.id, true)
        .await
        .expect("accept eula");

    // Keep the test off the default port and small.
    std::fs::write(
        inst.path_buf().join("server.properties"),
        "server-port=25599
max-players=1
online-mode=false
view-distance=4
".as_bytes(),
    )
    .unwrap();

    let reloaded = instance::get(&state.db, inst.id).await.unwrap();
    let java = mc_server_manager_lib::java::best_for(&state.db, 21)
        .await
        .unwrap()
        .expect("a Java 21+ runtime");
    let plan = launch::plan(&reloaded, std::path::Path::new(&java.path)).expect("launch plan");
    println!("launching: {:?} {:?}", plan.program, plan.args);

    let mut child = tokio::process::Command::new(&plan.program)
        .args(&plan.args)
        .current_dir(&plan.working_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the server");

    let pid = child.id().expect("pid");
    assert!(
        mc_server_manager_lib::process::supervisor::process_start_time(pid).is_some(),
        "the pid must be observable, which is what reconciliation relies on"
    );

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let mut lines = BufReader::new(stdout).lines();
    let mut buffer = ConsoleBuffer::new();

    let mut ready = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(180);
    while tokio::time::Instant::now() < deadline {
        let Ok(Some(raw)) = tokio::time::timeout(std::time::Duration::from_secs(30), lines.next_line())
            .await
            .unwrap_or(Ok(None))
        else {
            break;
        };

        let parsed = buffer.push(&raw, false);
        if let Some(LogEvent::Ready { took }) = logparse::detect_event(&parsed.message) {
            println!("server ready in {took:?} after {} lines", buffer.total_seen());
            ready = true;
            break;
        }
    }
    assert!(ready, "the server never reported being ready");

    // A command on stdin, then a graceful stop.
    stdin.write_all(b"say hello from the manager\n").await.unwrap();
    stdin.flush().await.unwrap();
    stdin.write_all(b"stop\n").await.unwrap();
    stdin.flush().await.unwrap();

    let mut saw_stopping = false;
    while let Ok(Some(raw)) = lines.next_line().await {
        let parsed = buffer.push(&raw, false);
        if matches!(logparse::detect_event(&parsed.message), Some(LogEvent::Stopping)) {
            saw_stopping = true;
        }
    }

    let status = tokio::time::timeout(std::time::Duration::from_secs(60), child.wait())
        .await
        .expect("the server exited within a minute")
        .expect("wait");

    assert!(saw_stopping, "the shutdown was never announced");
    assert_eq!(status.code(), Some(0), "a stop command exits cleanly");
    assert_eq!(
        mc_server_manager_lib::process::supervisor::process_start_time(pid),
        None,
        "the pid is gone once the server exits"
    );
    println!("captured {} console lines", buffer.total_seen());
}

/// The encoding question, answered by the server itself: write a MOTD with
/// non-ASCII characters, start Paper, let it rewrite `server.properties` on
/// boot, and check the value came back intact.
#[tokio::test]
#[ignore = "downloads Paper and runs a real server"]
async fn a_non_ascii_motd_survives_a_real_server_start() {
    use std::collections::BTreeMap;
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    use mc_server_manager_lib::config::{self, PropertiesUpdate};
    use mc_server_manager_lib::logparse::{self, LogEvent};
    use mc_server_manager_lib::process::launch;

    const MOTD: &str = "Čajovna — žíznivý šnek";

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let state = state_in(&root).await;
    mc_server_manager_lib::java::rescan(&state.db).await.unwrap();

    let inst = instance_in(&state, &root, "props", ServerType::Paper, "1.21.4").await;
    let cancel = CancellationToken::new();
    install::install(&state, &state.http, &inst, "1.21.4", None, &cancel, |_, _, _, _| {})
        .await
        .expect("install paper");
    mc_server_manager_lib::instance::eula::set(&state, inst.id, true)
        .await
        .unwrap();

    // A first run to make the server write its own server.properties.
    run_until_ready(&state, inst.id, Some(25602)).await;

    // Now edit it the way the UI does, and record what the file looked like.
    let before = std::fs::read_to_string(inst.path_buf().join("server.properties")).unwrap();
    let report = config::save(
        &state,
        inst.id,
        PropertiesUpdate {
            changes: BTreeMap::from([("motd".to_string(), MOTD.to_string())]),
        },
    )
    .await
    .expect("save properties");
    assert_eq!(report.changed, vec!["motd"]);
    assert!(report.backup_created, "the original file was kept");

    // Everything except the motd line is untouched.
    let after = std::fs::read_to_string(inst.path_buf().join("server.properties")).unwrap();
    let changed_lines: Vec<&str> = after
        .lines()
        .filter(|line| !before.lines().any(|old| old == *line))
        .collect();
    assert_eq!(changed_lines.len(), 1, "only one line changed: {changed_lines:?}");
    assert!(changed_lines[0].contains(MOTD));

    // The server reads and rewrites the file on boot: if the encoding were
    // wrong, this is where the MOTD would come back mangled.
    run_until_ready(&state, inst.id, None).await;

    let rewritten = config::read(&inst.path_buf()).await.unwrap();
    assert_eq!(
        rewritten.get("motd"),
        Some(MOTD),
        "the server preserved the MOTD it read from our file"
    );

    let bytes = std::fs::read(inst.path_buf().join("server.properties")).unwrap();
    assert!(
        std::str::from_utf8(&bytes).is_ok(),
        "the server wrote it back as UTF-8, matching how we wrote it"
    );
    println!("motd survived a real server start: {MOTD}");

    /// Starts the server, waits for "Done", then stops it cleanly.
    async fn run_until_ready(
        state: &AppState,
        id: i64,
        port: Option<u16>,
    ) {
        let row = instance::get(&state.db, id).await.unwrap();
        if let Some(port) = port {
            std::fs::write(
                row.path_buf().join("server.properties"),
                format!("server-port={port}\nmax-players=1\nonline-mode=false\nview-distance=4\n"),
            )
            .unwrap();
        }

        let java = mc_server_manager_lib::java::best_for(&state.db, 21)
            .await
            .unwrap()
            .expect("a Java 21+ runtime");
        let plan = launch::plan(&row, std::path::Path::new(&java.path)).unwrap();

        let mut child = tokio::process::Command::new(&plan.program)
            .args(&plan.args)
            .current_dir(&plan.working_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn");

        let mut stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut lines = BufReader::new(stdout).lines();

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(240);
        while tokio::time::Instant::now() < deadline {
            let Ok(Ok(Some(raw))) =
                tokio::time::timeout(std::time::Duration::from_secs(60), lines.next_line()).await
            else {
                break;
            };
            let (_, _, _, message) = logparse::parse_line(&raw, false);
            if matches!(logparse::detect_event(&message), Some(LogEvent::Ready { .. })) {
                break;
            }
        }

        stdin.write_all(b"stop\n").await.unwrap();
        stdin.flush().await.unwrap();
        while let Ok(Some(_)) = lines.next_line().await {}
        let status = tokio::time::timeout(std::time::Duration::from_secs(90), child.wait())
            .await
            .expect("the server exited")
            .unwrap();
        assert_eq!(status.code(), Some(0), "the server stopped cleanly");
    }
}

/// Modrinth, live: the identifying User-Agent is accepted, the budget headers
/// are read, search is filtered by loader and version, and a real dependency
/// tree resolves.
#[tokio::test]
#[ignore = "hits the Modrinth API"]
async fn modrinth_search_and_dependency_resolution_work_live() {
    use mc_server_manager_lib::mods::modrinth::Modrinth;
    use mc_server_manager_lib::mods::ratelimit::RateLimiter;
    use mc_server_manager_lib::mods::resolve::{self, Installed};
    use mc_server_manager_lib::mods::source::{Loader, ModSource, SearchQuery, VersionFilter};

    let dir = tempfile::tempdir().unwrap();
    let state = state_in(dir.path()).await;
    let index = providers::index::refresh(&state.db, &state.http).await.unwrap();

    let limiter = std::sync::Arc::new(RateLimiter::default());
    let modrinth = Modrinth::new(limiter.clone()).unwrap();

    let results = modrinth
        .search(&SearchQuery {
            sort: Default::default(),
            categories: Vec::new(),
            content_type: Default::default(),
            text: "lithium".into(),
            loaders: vec!["fabric".into()],
            game_versions: vec!["1.21.4".into()],
            limit: Some(5),
            offset: None,
        })
        .await
        .expect("search");

    assert!(!results.projects.is_empty(), "search returned nothing");
    assert!(results.total.unwrap_or(0) > 0, "and it says how many there are");
    let results = results.projects;
    println!(
        "search: {}",
        results
            .iter()
            .map(|project| project.title.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    // The budget headers were read and recorded by the shared limiter.
    let budget = limiter
        .budget("api.modrinth.com")
        .expect("Modrinth publishes a rate limit budget");
    println!("budget: {} remaining, resets in {}s", budget.remaining, budget.reset_in);
    assert!(budget.remaining > 0);

    // Waystones needs Fabric API and Balm: a real tree, resolved live.
    let versions = modrinth
        .versions(
            "LOpKHB2A",
            &VersionFilter {
                loaders: vec!["fabric".into()],
                game_versions: vec!["1.21.4".into()],
            },
        )
        .await
        .expect("versions");
    let root = resolve::pick_version(&versions, Loader::Fabric, "1.21.4", &index)
        .expect("a Waystones build for 1.21.4");

    let plan = resolve::plan(
        &modrinth,
        root,
        Loader::Fabric,
        "1.21.4",
        &index,
        &Installed::default(),
    )
    .await
    .expect("resolve");

    println!(
        "plan: {}",
        plan.install
            .iter()
            .map(|planned| format!("{} {}", planned.project_title, planned.version_number))
            .collect::<Vec<_>>()
            .join(", ")
    );
    assert!(plan.install.len() >= 3, "the tree should pull in dependencies");
    assert!(plan
        .install
        .iter()
        .all(|planned| planned.file_name.ends_with(".jar")));
    assert!(plan.total_size > 0);
}

/// Installing a mod for real: the jar lands in the right folder for the server
/// type, its SHA-512 verifies, and the row records where it came from.
#[tokio::test]
#[ignore = "downloads mods from Modrinth"]
async fn a_mod_installs_into_the_right_folder() {
    use mc_server_manager_lib::mods::{self, modrinth::Modrinth, ratelimit::RateLimiter};
    use mc_server_manager_lib::mods::resolve::{self, Installed};
    use mc_server_manager_lib::mods::source::{Loader, ModSource, VersionFilter};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let state = state_in(&root).await;
    let index = providers::index::refresh(&state.db, &state.http).await.unwrap();

    // A Fabric instance loads mods/, so that is where the jar must go.
    let inst = instance_in(&state, &root, "fabric", ServerType::Fabric, "1.21.4").await;
    let modrinth = Modrinth::new(std::sync::Arc::new(RateLimiter::default())).unwrap();

    let versions = modrinth
        .versions(
            "gvQqBUqZ", // Lithium: server-side, small, no dependencies.
            &VersionFilter {
                loaders: vec!["fabric".into()],
                game_versions: vec!["1.21.4".into()],
            },
        )
        .await
        .unwrap();
    let version = resolve::pick_version(&versions, Loader::Fabric, "1.21.4", &index).unwrap();

    let plan = resolve::plan(
        &modrinth,
        version.clone(),
        Loader::Fabric,
        "1.21.4",
        &index,
        &Installed::default(),
    )
    .await
    .unwrap();

    let planned = &plan.install[0];
    mods::install_planned(&state, inst.id, planned, &version, &CancellationToken::new())
        .await
        .expect("install");

    let jar = inst.path_buf().join("mods").join(&planned.file_name);
    assert!(jar.is_file(), "the jar is in mods/: {}", jar.display());
    assert!(!inst.path_buf().join("plugins").exists(), "no plugins folder for Fabric");

    let view = mods::list(&state, inst.id).await.unwrap();
    assert_eq!(view.content_dir.as_deref(), Some("mods"));
    let installed = view
        .mods
        .iter()
        .find(|entry| entry.file_name == planned.file_name)
        .expect("the mod is listed");
    assert!(installed.enabled);
    assert_eq!(
        installed.tracked.as_ref().and_then(|row| row.project_id.clone()),
        Some("gvQqBUqZ".to_string())
    );
    // The jar's own metadata was read back out of the file.
    assert!(installed.metadata.is_some(), "fabric.mod.json was read");
    assert!(installed.mismatch.is_none(), "{:?}", installed.mismatch);

    // Disabling renames it, and the server would ignore it.
    mods::set_enabled(&state, inst.id, &planned.file_name, false)
        .await
        .unwrap();
    assert!(!jar.exists());
    assert!(jar.with_file_name(format!("{}.disabled", planned.file_name)).is_file());
}


/// Phase 6, part one: a real Paper world survives an archive and a restore.
///
/// The instance is a genuine Paper install with a world Minecraft itself
/// generated — region files, `level.dat`, the lot — so this checks the archive
/// against the data the app actually has to protect rather than against files a
/// test wrote.
#[tokio::test]
#[ignore = "downloads Paper and runs a real server"]
async fn a_real_world_survives_a_backup_and_restore() {
    use mc_server_manager_lib::backup::archive::{Format, Scope};
    use mc_server_manager_lib::backup::{self, BackupOptions};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let state = state_in(&root).await;
    mc_server_manager_lib::java::rescan(&state.db).await.unwrap();

    let inst = instance_in(&state, &root, "backupme", ServerType::Paper, "1.21.4").await;
    let cancel = CancellationToken::new();
    install::install(&state, &state.http, &inst, "1.21.4", None, &cancel, |_, _, _, _| {})
        .await
        .expect("install paper");
    mc_server_manager_lib::instance::eula::set(&state, inst.id, true)
        .await
        .expect("accept eula");

    // Run the server once, purely to make it generate a world.
    let world_dir = inst.path_buf().join("world");
    run_server_until_ready(&state, &inst, 25598, &["save-all flush", "stop"]).await;
    assert!(world_dir.join("level.dat").is_file(), "the server generated a world");

    let marker = world_dir.join("msm-marker.txt");
    std::fs::write(&marker, b"phase 6 was here").unwrap();
    let level_dat = std::fs::read(world_dir.join("level.dat")).unwrap();
    let regions: Vec<PathBuf> = std::fs::read_dir(world_dir.join("region"))
        .expect("region folder")
        .flatten()
        .map(|entry| entry.path())
        .collect();
    assert!(!regions.is_empty(), "the world has region files to protect");

    for format in [Format::TarZst, Format::Zip] {
        let created = backup::create(
            &state,
            inst.id,
            BackupOptions {
                format,
                scope: Scope::Full,
                ..BackupOptions::default()
            },
            "manual",
            None,
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap_or_else(|err| panic!("{format:?} backup failed: {err}"));
        println!("{format:?}: {} ({} bytes)", created.path, created.size_bytes);

        // Wipe the world, then put it back from the archive.
        std::fs::remove_dir_all(&world_dir).expect("delete the world");
        backup::restore(&state, created.id, &CancellationToken::new(), |_| {})
            .await
            .unwrap_or_else(|err| panic!("{format:?} restore failed: {err}"));

        assert_eq!(
            std::fs::read_to_string(&marker).expect("the marker came back"),
            "phase 6 was here",
            "{format:?}"
        );
        assert_eq!(
            std::fs::read(world_dir.join("level.dat")).expect("level.dat came back"),
            level_dat,
            "{format:?}: level.dat is byte-identical"
        );
        for region in &regions {
            assert!(region.is_file(), "{format:?}: {} came back", region.display());
        }

        // The state that was replaced was archived first.
        assert!(
            backup::list(&state, inst.id)
                .await
                .unwrap()
                .iter()
                .any(|entry| entry.kind == "pre_restore"),
            "{format:?}: a safety copy was taken before overwriting"
        );
    }
}

/// Phase 6, part two: the flush confirmation this app waits for is the one Paper
/// actually prints.
///
/// The live-backup sequence hangs for two minutes and then reports a failure if
/// this matching is wrong, so it is checked against a real server's output
/// rather than against a remembered string.
#[tokio::test]
#[ignore = "downloads Paper and runs a real server"]
async fn a_real_server_confirms_save_off_and_save_on() {
    use mc_server_manager_lib::backup::saveguard;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let state = state_in(&root).await;
    mc_server_manager_lib::java::rescan(&state.db).await.unwrap();

    let inst = instance_in(&state, &root, "saveguard", ServerType::Paper, "1.21.4").await;
    let cancel = CancellationToken::new();
    install::install(&state, &state.http, &inst, "1.21.4", None, &cancel, |_, _, _, _| {})
        .await
        .expect("install paper");
    mc_server_manager_lib::instance::eula::set(&state, inst.id, true)
        .await
        .expect("accept eula");

    // The exact sequence a backup of a running server sends.
    let transcript =
        run_server_until_ready(&state, &inst, 25597, &["save-off", "save-all flush", "save-on", "stop"])
            .await;

    let confirmed: Vec<&String> = transcript
        .iter()
        .filter(|line| saveguard::flush_confirmed(line))
        .collect();
    assert!(
        !confirmed.is_empty(),
        "no line matched the flush confirmation; the live backup would wait for two minutes.\n{}",
        transcript.join("\n")
    );
    println!("flush confirmed by: {confirmed:?}");

    assert!(
        transcript.iter().any(|line| line.contains("saving is now disabled")
            || line.contains("Turned off world auto-saving")),
        "the server acknowledged save-off"
    );
    assert!(
        transcript.iter().any(|line| line.contains("saving is now enabled")
            || line.contains("Turned on world auto-saving")),
        "the server acknowledged save-on"
    );
}

/// Starts the instance's server for real, waits for it to report ready, sends
/// `commands` on stdin and returns every parsed console message.
///
/// The supervisor itself needs an `AppHandle`, which a test cannot build, so
/// this drives the same launch plan the supervisor would.
async fn run_server_until_ready(
    state: &AppState,
    inst: &mc_server_manager_lib::db::models::Instance,
    port: u16,
    commands: &[&str],
) -> Vec<String> {
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    use mc_server_manager_lib::logparse::{self, LogEvent};
    use mc_server_manager_lib::process::console::ConsoleBuffer;
    use mc_server_manager_lib::process::launch;

    std::fs::write(
        inst.path_buf().join("server.properties"),
        format!("server-port={port}\nmax-players=1\nonline-mode=false\nview-distance=4\n").as_bytes(),
    )
    .unwrap();

    let reloaded = instance::get(&state.db, inst.id).await.unwrap();
    let java = mc_server_manager_lib::java::best_for(&state.db, 21)
        .await
        .unwrap()
        .expect("a Java 21+ runtime");
    let plan = launch::plan(&reloaded, Path::new(&java.path)).expect("launch plan");

    let mut child = tokio::process::Command::new(&plan.program)
        .args(&plan.args)
        .current_dir(&plan.working_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the server");

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let mut lines = BufReader::new(stdout).lines();
    let mut buffer = ConsoleBuffer::new();
    let mut transcript = Vec::new();

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
    let mut ready = false;
    while tokio::time::Instant::now() < deadline {
        let Ok(Some(raw)) =
            tokio::time::timeout(std::time::Duration::from_secs(60), lines.next_line())
                .await
                .unwrap_or(Ok(None))
        else {
            break;
        };
        let parsed = buffer.push(&raw, false);
        transcript.push(parsed.message.clone());
        if let Some(LogEvent::Ready { took }) = logparse::detect_event(&parsed.message) {
            println!("ready in {took:?}");
            ready = true;
            break;
        }
    }
    assert!(ready, "the server never reported being ready:\n{}", transcript.join("\n"));

    for command in commands {
        stdin.write_all(format!("{command}\n").as_bytes()).await.unwrap();
        stdin.flush().await.unwrap();
        // Give the server a moment to answer before the next one, so the
        // transcript shows each acknowledgement in order.
        tokio::time::sleep(std::time::Duration::from_millis(750)).await;
    }

    while let Ok(Ok(Some(raw))) =
        tokio::time::timeout(std::time::Duration::from_secs(60), lines.next_line()).await
    {
        transcript.push(buffer.push(&raw, false).message);
    }

    let _ = tokio::time::timeout(std::time::Duration::from_secs(60), child.wait()).await;
    transcript
}

/// Reproduction harness: builds the launch command for a real instance folder on
/// this machine and prints it, then runs the JVM with those exact arguments.
///
/// Kept ignored like the rest of this file. It exists because "which arguments
/// did it actually spawn" was guessed at three times before anybody printed it.
#[tokio::test]
#[ignore = "reads a folder on the developer's machine"]
async fn the_real_instance_on_this_machine_produces_a_command_that_runs() {
    use mc_server_manager_lib::db::models::{Instance, LaunchKind};
    use mc_server_manager_lib::process::launch;

    // Point this at any instance folder: `MSM_REPRO_INSTANCE=... cargo test ...`.
    let Ok(folder) = std::env::var("MSM_REPRO_INSTANCE") else {
        println!("set MSM_REPRO_INSTANCE to an instance folder to run this");
        return;
    };
    let dir = PathBuf::from(&folder);
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join(".msm").join("instance.json")).expect("instance.json"),
    )
    .expect("valid manifest");

    let dir_for_state = tempfile::tempdir().unwrap();
    let state = state_in(dir_for_state.path()).await;
    let now = mc_server_manager_lib::db::now_rfc3339();
    sqlx::query(
        "INSERT INTO instances (uuid, name, path, server_type, mc_version, launch_kind,
            launch_target, jvm_args, server_args, min_ram_mb, max_ram_mb, eula_accepted,
            installed_at, created_at, updated_at)
         VALUES (?, ?, ?, 'fabric', ?, 'jar', ?, ?, ?, ?, ?, 1, ?, ?, ?)",
    )
    .bind(manifest["uuid"].as_str().unwrap())
    .bind(manifest["name"].as_str().unwrap())
    .bind(&folder)
    .bind(manifest["mcVersion"].as_str().unwrap())
    .bind(manifest["launchTarget"].as_str().unwrap())
    .bind(manifest["jvmArgs"].to_string())
    .bind(manifest["serverArgs"].to_string())
    .bind(manifest["minRamMb"].as_i64().unwrap())
    .bind(manifest["maxRamMb"].as_i64().unwrap())
    .bind(&now)
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await
    .unwrap();

    let row: Instance = instance::get(&state.db, 1).await.unwrap();
    assert_eq!(row.launch_kind, LaunchKind::Jar);

    mc_server_manager_lib::java::rescan(&state.db).await.unwrap();
    let required = row
        .java_major
        .unwrap_or_else(|| mc_server_manager_lib::java::required_java_for(&row.mc_version));
    let chosen = mc_server_manager_lib::java::best_for(&state.db, required)
        .await
        .unwrap()
        .expect("a usable runtime");
    println!("required Java {required}, chose {} (bits {:?})", chosen.path, chosen.bits);

    let plan = launch::plan(&row, Path::new(&chosen.path)).expect("plan");
    println!("argv: {}", launch::quoted_command(&plan.program, &plan.args));
    launch::validate_args(&plan.args).expect("the command line is well formed");

    // The real thing: run that JVM with those arguments, up to -version.
    let mut args = plan.args.clone();
    args.truncate(args.iter().position(|a| a == "-jar").unwrap_or(args.len()));
    args.push("-version".to_string());
    let output = std::process::Command::new(&plan.program)
        .args(&args)
        .output()
        .expect("run the JVM");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    println!("{text}");
    assert!(
        !text.contains("Invalid maximum heap size"),
        "the chosen JVM refuses this command line"
    );
}

/// Downloads a real JDK from Adoptium, unpacks it, and runs it.
///
/// The whole managed-runtime path end to end: resolve, verify the checksum,
/// extract into place, then ask the binary what it is. Java 17 is used because
/// it is the smallest current line.
#[tokio::test]
#[ignore = "downloads ~180 MB from Adoptium"]
async fn a_managed_jdk_downloads_unpacks_and_reports_its_version() {
    use mc_server_manager_lib::java::adoptium;
    use mc_server_manager_lib::java::managed;

    let dir = tempfile::tempdir().unwrap();
    let state = state_in(dir.path()).await;

    let candidate = adoptium::resolve(
        &state.http,
        17,
        adoptium::current_os(),
        adoptium::current_arch(),
    )
    .await
    .expect("resolve a JDK 17");
    println!(
        "{} {} ({} MB) {}",
        candidate.release_name,
        candidate.openjdk_version,
        candidate.size_bytes / 1_048_576,
        candidate.url
    );
    assert_eq!(candidate.feature_version, 17);
    assert_eq!(candidate.sha256.len(), 64);

    let cancel = CancellationToken::new();
    let runtime = managed::install(&state, &candidate, &cancel, |progress| {
        if progress.downloaded % (32 * 1_048_576) < 65_536 {
            println!("  {} MB", progress.downloaded / 1_048_576);
        }
    })
    .await
    .expect("install the JDK");

    // It landed where every instance can share it, not inside one instance.
    let expected_dir = managed::install_dir(dir.path(), 17);
    assert!(
        Path::new(&runtime.java_path).starts_with(&expected_dir),
        "{} is not under {}",
        runtime.java_path,
        expected_dir.display()
    );
    assert!(Path::new(&runtime.java_path).is_file());
    assert!(runtime.size_bytes > 50_000_000, "{}", runtime.size_bytes);

    // No staging folder survived the install.
    let leftovers: Vec<PathBuf> = std::fs::read_dir(managed::runtimes_dir(dir.path()))
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.file_name().is_some_and(|name| {
            name.to_string_lossy().starts_with(".staging")
        }))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    // And the binary really is Java 17.
    let major = mc_server_manager_lib::java::probe_major(Path::new(&runtime.java_path))
        .await
        .expect("the downloaded binary answers -version");
    assert_eq!(major, 17);

    // It is registered, listed, and selected ahead of any system JDK.
    let listed = managed::list(&state).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].feature_version, 17);
    assert!(managed::total_size(&state).await.unwrap() > 50_000_000);

    let chosen = mc_server_manager_lib::java::select_for(&state, None, 17)
        .await
        .unwrap()
        .expect("something satisfies 17");
    assert_eq!(chosen.origin, mc_server_manager_lib::java::Origin::Managed);

    // Unused, so it can be removed again — files and row together.
    managed::remove(&state, 17).await.expect("remove it again");
    assert!(!expected_dir.exists());
    assert!(managed::list(&state).await.unwrap().is_empty());
}
