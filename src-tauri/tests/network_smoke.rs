//! Network smoke tests. Every one is `#[ignore]`, so CI stays offline and a bad
//! day at Mojang or PaperMC cannot break the build. Run them by hand when
//! touching providers, the downloader, or the installer:
//!
//! ```text
//! cargo test --test network_smoke -- --ignored --nocapture
//! ```

use std::path::Path;

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
