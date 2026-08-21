//! The managed-runtime path, walked end to end against the real world.
//!
//! Everything here has unit coverage against fixtures, and none of it had ever
//! run on a machine where the answer mattered: a system JDK that satisfies the
//! requirement hides the whole feature. A 1.16.5 server needs Java 8, which is
//! the one version a modern machine is least likely to have as a 64-bit build,
//! so it is the case that exercises the offer, the download and the selection.
//!
//! Live: hits the Adoptium API, downloads a JDK, and (in the second test) boots
//! a real Minecraft server on it. Both are `#[ignore]`.

use std::path::Path;

use mc_server_manager_lib::db;
use mc_server_manager_lib::db::models::ServerType;
use mc_server_manager_lib::instance::{self, crud, CreateInstanceInput};
use mc_server_manager_lib::java::{self, adoptium, managed, JavaFit, Origin};
use mc_server_manager_lib::state::AppState;
use tokio_util::sync::CancellationToken;

const MC_VERSION: &str = "1.16.5";
const REQUIRED_MAJOR: i64 = 8;

async fn state_in(dir: &Path) -> AppState {
    let pool = db::connect_in_memory().await.expect("in-memory database");
    AppState::new(pool, dir.to_path_buf())
}

/// The whole path, without launching a server: detection, the offer, the
/// download, selection, the dependants list, the delete refusal, and what
/// "use system Java only" does to all of it.
#[tokio::test]
#[ignore = "hits the Adoptium API and downloads a JDK (~100 MB)"]
async fn a_1_16_5_server_gets_java_8_from_the_app_and_keeps_it() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_in(dir.path()).await;

    // 1. What this machine really has. The scan is the app's own, so the answer
    //    is the one the picker would show.
    let detected = java::rescan(&state.db).await.expect("scan for Java");
    println!("\n=== detected Java on this machine ===");
    for runtime in &detected {
        println!(
            "  Java {:>2}  {:>7}  {}{}",
            runtime.major,
            match runtime.bits {
                Some(bits) => format!("{bits}-bit"),
                None => "unknown".to_string(),
            },
            runtime.path,
            match runtime.unsuitable_reason() {
                Some(reason) => format!("   [excluded: {reason}]"),
                None => String::new(),
            }
        );
    }

    let java_8s: Vec<_> = detected.iter().filter(|r| r.major == REQUIRED_MAJOR).collect();
    let usable_8s: Vec<_> = java_8s.iter().filter(|r| r.usable_for_servers()).collect();
    println!(
        "  Java 8 installs: {} found, {} usable for servers",
        java_8s.len(),
        usable_8s.len()
    );
    for runtime in &java_8s {
        // The claim under test: a 32-bit Java 8 is excluded for its width, not
        // offered and left to fail at launch with an unreadable heap error.
        if !runtime.usable_for_servers() {
            println!(
                "  excluded by width, before any launch: {} ({})",
                runtime.path,
                runtime.unsuitable_reason().unwrap_or("no reason recorded")
            );
            assert_eq!(
                runtime.bits,
                Some(32),
                "the exclusion has to be about width, not about the probe failing"
            );
        }
    }

    // 2. An instance that needs Java 8.
    let row = crud::create(
        &state,
        CreateInstanceInput {
            name: "managed-walk".into(),
            path: dir.path().join("managed-walk").to_string_lossy().to_string(),
            server_type: ServerType::Vanilla,
            mc_version: MC_VERSION.into(),
            loader_version: None,
            min_ram_mb: Some(1024),
            max_ram_mb: Some(1024),
            notes: None,
            color: None,
            web_map: false,
        },
    )
    .await
    .expect("create the instance");

    let required = java::required_for(row.java_major, MC_VERSION);
    assert_eq!(required, REQUIRED_MAJOR, "1.16.5 asks for Java 8");

    // Vanilla takes anything newer; the loaders want this exact major.
    let fit = java::fit_for(ServerType::Vanilla);
    assert_eq!(fit, JavaFit::Floor);

    // 3. What the create dialog would show before anything is downloaded.
    //
    // The requirement is a floor, so any usable JDK at or above it satisfies
    // the instance and the offer stays hidden. On a machine with a newer JDK
    // and no 64-bit Java 8, that means the offer is not reachable through this
    // version at all — which is worth printing rather than asserting away.
    let before = java::select_for(&state, None, required, fit).await.unwrap();
    match &before {
        None => println!("\nnothing installed satisfies Java 8, so the app offers a download"),
        Some(selection) => println!(
            "\nthe floor is already satisfied by {} (Java {}), so no offer is made",
            selection.path.display(),
            selection.major.map(|m| m.to_string()).unwrap_or_else(|| "?".into()),
        ),
    }
    if let Some(selection) = &before {
        let major = java::probe_major(&selection.path).await;
        assert!(
            major.is_some_and(|major| major >= REQUIRED_MAJOR),
            "whatever satisfies the floor really is at or above it: {major:?}"
        );
        assert!(
            !java_8s
                .iter()
                .any(|runtime| Path::new(&runtime.path) == selection.path && !runtime.usable_for_servers()),
            "and it is never one of the 32-bit Java 8s"
        );
    }

    let candidate = adoptium::resolve(&state.http, required, adoptium::current_os(), adoptium::current_arch())
        .await
        .expect("Adoptium has a Java 8 build for this platform");
    println!(
        "offer: Temurin {} ({}), {:.1} MB",
        candidate.openjdk_version,
        candidate.release_name,
        candidate.size_bytes as f64 / 1_048_576.0
    );

    // 4. The download, through the Phase 2 engine.
    let cancel = CancellationToken::new();
    let installed = managed::install(&state, &candidate, &cancel, |progress| {
        if progress.downloaded == progress.total.unwrap_or(0) {
            println!("  downloaded {} bytes", progress.downloaded);
        }
    })
    .await
    .expect("install the managed runtime");

    println!(
        "installed: Java {} at {}",
        installed.feature_version, installed.java_path
    );
    assert!(Path::new(&installed.java_path).is_file(), "the binary is on disk");

    // The binary really is Java 8, asked directly rather than assumed.
    let probed = java::probe_major(Path::new(&installed.java_path)).await;
    assert_eq!(probed, Some(REQUIRED_MAJOR), "the runtime answers -version as Java 8");

    // 5. Selection now prefers it, and says where it came from.
    let chosen = java::select_for(&state, None, required, fit)
        .await
        .unwrap()
        .expect("something is selected now");
    println!("selection: {:?} {}", chosen.origin, chosen.path.display());
    assert_eq!(chosen.origin, Origin::Managed, "the app's own runtime comes first");
    assert_eq!(chosen.path, Path::new(&installed.java_path));

    // 6. The Settings list: size on disk, and who would break without it.
    let listed = managed::list(&state).await.unwrap();
    let entry = listed
        .iter()
        .find(|runtime| runtime.feature_version == REQUIRED_MAJOR)
        .expect("the runtime is listed");
    let total = managed::total_size(&state).await.unwrap();
    println!(
        "listed: Java {} · {:.0} MB on disk · used by {:?}",
        entry.feature_version,
        entry.size_bytes as f64 / 1_048_576.0,
        entry.used_by
    );
    assert!(entry.size_bytes > 0, "a size is reported");
    assert_eq!(total, entry.size_bytes, "and it is the whole total here");
    assert!(
        entry.used_by.iter().any(|name| name == &row.name),
        "the dependent instance is named: {:?}",
        entry.used_by
    );

    // 7. Deleting it is refused, and the refusal names the server.
    let refusal = managed::remove(&state, REQUIRED_MAJOR)
        .await
        .expect_err("removal is refused while an instance depends on it");
    println!("refusal: {refusal}");
    assert!(
        refusal.to_string().contains(&row.name),
        "the refusal names the server: {refusal}"
    );
    assert!(
        Path::new(&installed.java_path).is_file(),
        "and nothing was deleted"
    );

    // 8. "Use only the Java installed on this computer" — the managed runtime
    //    stays on disk but stops being an answer, so the app refuses rather
    //    than quietly using it against the setting.
    db::setting_set(&state.db, managed::SYSTEM_ONLY_SETTING, "true")
        .await
        .unwrap();
    assert!(!managed::downloads_allowed(&state).await);

    let with_system_only = java::select_for(&state, None, required, fit).await.unwrap();
    match &with_system_only {
        None => println!("system-only: nothing suitable, so the app refuses"),
        Some(selection) => println!("system-only: fell back to {}", selection.path.display()),
    }
    assert_ne!(
        with_system_only.as_ref().map(|selection| selection.origin),
        Some(Origin::Managed),
        "system-only must never answer with a runtime this app downloaded"
    );
    match (&before, &with_system_only) {
        // Nothing satisfied the floor before the download, so nothing does now.
        (None, after) => assert!(
            after.is_none(),
            "with no system Java at all, system-only has to mean no selection"
        ),
        // A system JDK satisfied it before; it is what the app goes back to.
        (Some(_), after) => assert_eq!(
            after.as_ref().map(|selection| selection.origin),
            Some(Origin::System),
            "and where a system JDK satisfies the floor, that is the fallback"
        ),
    }

    // Back off again, so the last step can clean up.
    db::setting_set(&state.db, managed::SYSTEM_ONLY_SETTING, "false")
        .await
        .unwrap();

    // 8b. The same version under a mod loader. This is the case the rule
    //     exists for: a system Java 17 satisfies a vanilla 1.16.5 server and
    //     must not be handed to a Forge one.
    let loader_fit = java::fit_for(ServerType::Forge);
    assert_eq!(loader_fit, JavaFit::Exact);

    let with_managed = java::select_for(&state, None, required, loader_fit)
        .await
        .unwrap()
        .expect("the managed Java 8 fits a loader exactly");
    println!(
        "loader with a managed Java 8: {:?} {}",
        with_managed.origin,
        with_managed.path.display()
    );
    assert_eq!(with_managed.origin, Origin::Managed);

    // With downloads off, the loader is left with the system list — where the
    // only Java 8s are 32-bit and everything else is too new to substitute.
    db::setting_set(&state.db, managed::SYSTEM_ONLY_SETTING, "true")
        .await
        .unwrap();
    let loader_system_only = java::select_for(&state, None, required, loader_fit)
        .await
        .unwrap();
    match &loader_system_only {
        None => println!("loader, system-only: nothing fits, so the app offers Java 8"),
        Some(selection) => println!(
            "loader, system-only: {} (Java {:?})",
            selection.path.display(),
            selection.major
        ),
    }
    if usable_8s.is_empty() {
        assert!(
            loader_system_only.is_none(),
            "a loader must not be given a newer Java in place of the one it wants"
        );
    }
    db::setting_set(&state.db, managed::SYSTEM_ONLY_SETTING, "false")
        .await
        .unwrap();

    // 9. With the instance gone, the same delete is allowed.
    crud::delete(&state, row.id, false).await.expect("delete the instance");
    managed::remove(&state, REQUIRED_MAJOR)
        .await
        .expect("removal is allowed once nothing depends on it");
    assert!(
        !Path::new(&installed.java_path).exists(),
        "and the folder is gone"
    );
    println!("removed once nothing depended on it\n");
}

/// The same runtime, actually running a server.
///
/// Separate because it downloads Mojang's 1.16.5 server and generates a world;
/// the point is the one line at the end saying which binary it ran on.
#[tokio::test]
#[ignore = "downloads a JDK and a Minecraft server, then boots it"]
async fn a_1_16_5_server_boots_on_the_managed_java_8() {
    use mc_server_manager_lib::instance::install;
    use mc_server_manager_lib::logparse::{self, LogEvent};
    use mc_server_manager_lib::process::launch;
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let dir = tempfile::tempdir().unwrap();
    let state = state_in(dir.path()).await;
    java::rescan(&state.db).await.unwrap();

    let row = crud::create(
        &state,
        CreateInstanceInput {
            name: "managed-boot".into(),
            path: dir.path().join("managed-boot").to_string_lossy().to_string(),
            server_type: ServerType::Vanilla,
            mc_version: MC_VERSION.into(),
            loader_version: None,
            min_ram_mb: Some(1024),
            max_ram_mb: Some(1024),
            notes: None,
            color: None,
            web_map: false,
        },
    )
    .await
    .unwrap();

    let cancel = CancellationToken::new();
    install::install(&state, &state.http, &row, MC_VERSION, None, &cancel, |_, _, _, _| {})
        .await
        .expect("install the server jar");

    let candidate = adoptium::resolve(
        &state.http,
        REQUIRED_MAJOR,
        adoptium::current_os(),
        adoptium::current_arch(),
    )
    .await
    .unwrap();
    let runtime = managed::install(&state, &candidate, &cancel, |_| {})
        .await
        .expect("install the managed runtime");

    let row = instance::get(&state.db, row.id).await.unwrap();
    let required = java::required_for(row.java_major, MC_VERSION);
    let chosen = java::select_for(
        &state,
        row.java_path.as_deref(),
        required,
        java::fit_for(row.server_type),
    )
    .await
    .unwrap()
    .expect("a runtime");
    assert_eq!(chosen.origin, Origin::Managed);

    // The EULA, written by the test rather than by the app: nothing in the
    // application writes this file without a click, and that stays true here.
    std::fs::write(row.path_buf().join("eula.txt"), "eula=true\n").unwrap();
    std::fs::write(
        row.path_buf().join("server.properties"),
        "server-port=25599\nmax-players=1\nonline-mode=false\nview-distance=4\n",
    )
    .unwrap();

    let plan = launch::plan(&row, &chosen.path).unwrap();
    println!("\nlaunching: {}", launch::quoted_command(&plan.program, &plan.args));

    let mut child = tokio::process::Command::new(&plan.program)
        .args(&plan.args)
        .current_dir(&plan.working_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the server");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut lines = BufReader::new(stdout).lines();

    let mut ready = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
    while tokio::time::Instant::now() < deadline {
        let Ok(Ok(Some(raw))) =
            tokio::time::timeout(std::time::Duration::from_secs(90), lines.next_line()).await
        else {
            break;
        };
        let (_, _, _, message) = logparse::parse_line(&raw, false);
        if matches!(logparse::detect_event(&message), Some(LogEvent::Ready { .. })) {
            ready = true;
            println!("server ready on {}", runtime.java_path);
            break;
        }
    }
    assert!(ready, "the server reached its Done line");

    stdin.write_all(b"stop\n").await.unwrap();
    stdin.flush().await.unwrap();
    while let Ok(Some(_)) = lines.next_line().await {}
    let status = tokio::time::timeout(std::time::Duration::from_secs(120), child.wait())
        .await
        .expect("the server exited")
        .unwrap();
    assert_eq!(status.code(), Some(0), "and it stopped cleanly");
}
