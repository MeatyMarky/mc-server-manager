//! The web map: squaremap, installed as a mod rather than rendered here.
//!
//! squaremap has spent years on chunk rendering and browser tiles, and it is
//! the light one — 2D Leaflet tiles that look like the game, no WebGL in the
//! webview, quick to a usable map, small on disk, and nothing to accept before
//! it will run. This app installs it and gets out of the way.
//!
//! One map means one config format, one URL shape and one render command, so
//! every path here is a path that actually runs. The port is *read*, never
//! assumed: squaremap writes a config the user is free to edit, and a Map tab
//! pointing at the port we hoped for is worse than no Map tab at all.

pub mod config;

use std::path::Path;

use serde::Serialize;
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

use crate::db::models::{Instance, ServerType};
use crate::error::{AppError, AppResult};
use crate::mods::source::{ModSource, SourceId, VersionFilter};
use crate::state::AppState;

/// The Modrinth project, so installation goes through the existing source path
/// rather than a second downloader.
pub const SLUG: &str = "squaremap";
/// Lowercase, which is how the project writes its own name.
pub const LABEL: &str = "squaremap";
/// What squaremap opens on when nothing else has been arranged. Only ever the
/// starting point for finding a free port.
pub const DEFAULT_PORT: u16 = 8080;
/// The value stored in `instances.map_kind`.
const STORED: &str = "squaremap";

/// Whether squaremap runs on this kind of server.
///
/// Fabric, NeoForge and the Bukkit family. Forge is left out on purpose:
/// squaremap's last Forge build is 1.2.0, for 1.20.1 alone, so offering it
/// would mostly offer an install that cannot resolve. Vanilla loads no mods at
/// all and is offered nothing rather than a failure.
pub fn supported(server_type: ServerType) -> bool {
    matches!(
        server_type,
        ServerType::Fabric | ServerType::NeoForge | ServerType::Paper | ServerType::Purpur
    )
}

/// The console command that renders a world already played.
///
/// squaremap renders chunks as they are loaded and saved, so a world played
/// before the map existed stays blank until it is asked. Sent from the console,
/// so no leading slash — and it names the world by dimension, because its
/// command parser takes a world identifier and `level-name` is not one.
pub fn render_command() -> String {
    "squaremap fullrender minecraft:overworld".to_string()
}

/// The address to open, centred on the world's spawn.
///
/// A map opened at 0,0 is opened at nothing in particular — the world is
/// wherever its spawn is, which may be thousands of blocks away. squaremap
/// takes the position as query parameters and names the world by dimension;
/// without a spawn (no level.dat yet) the plain address is returned rather than
/// a guessed position.
pub fn view_url(port: u16, spawn: Option<(i64, i64)>) -> String {
    let base = format!("http://127.0.0.1:{port}");
    match spawn {
        Some((x, z)) => format!("{base}/?world=minecraft_overworld&zoom=2&x={x}&z={z}"),
        None => base,
    }
}

/// The map mod found in an instance folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// The jar, so the UI can say what was found.
    pub file_name: String,
}

/// Whether squaremap is in this instance's content folder.
///
/// The jar name is the evidence: squaremap names its file after itself
/// (`squaremap-fabric-mc26.2-1.3.15.jar`), and a config folder alone can
/// survive an uninstall. A `.jar.disabled` counts: the mod is installed, the
/// user has just switched it off.
pub fn detect(instance: &Instance) -> AppResult<Option<Installed>> {
    let Ok(folder) = crate::mods::content_dir(instance) else {
        // A vanilla server has no content folder, and no map either.
        return Ok(None);
    };
    let Ok(entries) = std::fs::read_dir(&folder) else {
        return Ok(None);
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let lower = name.to_ascii_lowercase();
        if !(lower.ends_with(".jar") || lower.ends_with(".jar.disabled")) {
            continue;
        }
        if lower.starts_with(SLUG) {
            return Ok(Some(Installed { file_name: name }));
        }
    }

    Ok(None)
}

/// Everything the Map tab shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct MapStatus {
    /// True when squaremap is installed here.
    pub installed: bool,
    /// True when this server type could have it, for the offer when it has none.
    pub supported: bool,
    /// The port its config names. `None` until squaremap has written one, which
    /// happens on the server's first start after installing.
    pub port: Option<u16>,
    /// Where the config lives, so the UI can point at it rather than describe it.
    pub config_path: Option<String>,
    /// The address to open, once there is a port.
    pub url: Option<String>,
    /// True when this server is running, which is the only time a map answers.
    pub running: bool,
    /// Another server already using this port, named. Two maps on one port is
    /// the second one silently failing to start its web server.
    pub conflict: Option<String>,
    /// True when almost nothing has been rendered yet. A new map is a blank
    /// rectangle, which reads as broken rather than as empty.
    pub barely_rendered: bool,
    /// The world the server loads, for the hint.
    pub world: Option<String>,
    /// The address squaremap's web server binds to, as its own config says.
    pub bind: Option<String>,
    /// True when that bind address lets the rest of the network in. squaremap
    /// ships `0.0.0.0`, so the map is on the LAN from the first start — worth
    /// saying rather than presenting a loopback address as the whole truth.
    pub reaches_the_network: bool,
    /// The console command that renders what has already been played. Built
    /// here so the page never assembles a command.
    pub render_command: Option<String>,
}

/// Everything the Map tab needs, read fresh from disk.
pub async fn status(state: &AppState, id: i64) -> AppResult<MapStatus> {
    let row = crate::instance::get(&state.db, id).await?;
    let installed = detect(&row)?.is_some();

    let port = if installed {
        config::read_port(&row).await?
    } else {
        None
    };
    let bind = if installed {
        config::read_bind(&row).await?
    } else {
        None
    };

    // The world the server will load, which is what the hint names.
    let world = {
        let path = row.path_buf();
        tokio::task::spawn_blocking(move || {
            std::fs::read_to_string(crate::paths::server_properties_path(&path))
                .ok()
                .and_then(|text| crate::process::port::read_property(&text, "level-name"))
                .unwrap_or_else(|| "world".to_string())
        })
        .await
        .unwrap_or_else(|_| "world".to_string())
    };

    // Where the world starts, from the level.dat the Worlds tab already reads.
    let spawn = {
        let world_dir = row.path_buf().join(&world);
        let active = world.clone();
        tokio::task::spawn_blocking(move || {
            let found = crate::worlds::read_world(&world_dir, &active);
            found.spawn_x.zip(found.spawn_z)
        })
        .await
        .unwrap_or(None)
    };

    let barely_rendered = if installed {
        let path = row.path_buf();
        let server_type = row.server_type;
        tokio::task::spawn_blocking(move || barely_rendered(&path, server_type))
            .await
            .unwrap_or(false)
    } else {
        false
    };

    let conflict = match port {
        Some(port) => conflict_for(state, id, port).await?,
        None => None,
    };

    Ok(MapStatus {
        supported: supported(row.server_type),
        config_path: installed.then(|| config::config_path(&row).to_string_lossy().to_string()),
        // Loopback rather than a name: this is the machine the server runs on,
        // and the Networking tab is where the shareable addresses live.
        url: port.map(|port| view_url(port, spawn)),
        running: state.status_of(&row.uuid).is_live(),
        render_command: installed.then(render_command),
        reaches_the_network: bind
            .as_deref()
            .map(config::reaches_the_network)
            .unwrap_or(false),
        world: Some(world),
        bind,
        installed,
        port,
        conflict,
        barely_rendered,
    })
}

/// How full the tile folder has to be before the map stops counting as new.
///
/// squaremap writes one file per rendered tile, so a played-in world has
/// hundreds. A dozen is generous for "nothing has been rendered yet" and cheap
/// to count.
const RENDERED_ENOUGH: usize = 12;

/// Whether the map has rendered so little that it will look broken.
///
/// Counting stops at `RENDERED_ENOUGH`: the question is "is this empty", not
/// "how big is it", and a rendered world holds tens of thousands of files.
fn barely_rendered(instance_path: &Path, server_type: ServerType) -> bool {
    let root = config::tile_dir(instance_path, server_type);
    if !root.is_dir() {
        return true;
    }

    let mut found = 0usize;
    for entry in walkdir::WalkDir::new(&root)
        .max_depth(6)
        .into_iter()
        .filter_map(Result::ok)
    {
        if entry.file_type().is_file() {
            found += 1;
            if found >= RENDERED_ENOUGH {
                return false;
            }
        }
    }
    true
}

/// The name of another server that would fight this one for a port.
///
/// Both its game port and its map port count: a map on 25565 is as broken as
/// two maps on 8080, and the failure mode is the same — the second web server
/// quietly does not start, and the map tab shows an error page nobody can
/// explain.
pub async fn conflict_for(state: &AppState, id: i64, port: u16) -> AppResult<Option<String>> {
    for other in crate::instance::list(&state.db).await? {
        if other.id == id {
            continue;
        }
        let game_port = {
            let path = other.path_buf();
            tokio::task::spawn_blocking(move || crate::process::port::configured_port(&path))
                .await
                .unwrap_or(crate::process::port::DEFAULT_PORT)
        };
        if game_port == port {
            return Ok(Some(format!("{} (its game port)", other.name)));
        }
        if detect(&other)?.is_some() && config::read_port(&other).await? == Some(port) {
            return Ok(Some(format!("{} (its map)", other.name)));
        }
    }
    Ok(None)
}

/// A port no other server on this machine is using, starting from squaremap's
/// own default and counting up.
///
/// Checked against what is listening as well as against the other instances:
/// the map has to coexist with whatever else this computer runs, and a port
/// that is busy today is a map that silently fails on its first start.
pub async fn free_port(state: &AppState, id: i64) -> AppResult<u16> {
    for candidate in DEFAULT_PORT..DEFAULT_PORT.saturating_add(50) {
        if conflict_for(state, id, candidate).await?.is_some() {
            continue;
        }
        let taken =
            tokio::task::spawn_blocking(move || !crate::process::port::port_is_free(candidate))
                .await
                .unwrap_or(false);
        if !taken {
            return Ok(candidate);
        }
    }
    // Fifty in a row taken is not a case worth inventing behaviour for; the
    // default at least produces squaremap's own error rather than ours.
    Ok(DEFAULT_PORT)
}

/// Moves an installed map onto a port nothing else is using.
///
/// The repair for a map installed by hand, or one whose port something else has
/// taken since. The change takes effect on the next start, because squaremap
/// holds its config in memory while it runs.
pub async fn move_to_free_port(state: &AppState, id: i64) -> AppResult<Option<u16>> {
    let row = crate::instance::get(&state.db, id).await?;
    if detect(&row)?.is_none() {
        return Ok(None);
    }

    let port = free_port(state, id).await?;
    if config::read_port(&row).await? == Some(port) {
        return Ok(Some(port));
    }

    let moved = config::write_port(&row, port).await?;
    Ok(moved.then_some(port))
}

/// Installs squaremap and puts it on a port nothing else wants.
///
/// The install itself is the ordinary mod path — same source, same resolver,
/// same allowlisted download host — because a map is a mod. Its Fabric build
/// depends on Fabric API, and the resolver brings that along like any other
/// dependency. What this adds is choosing the project and settling the port.
pub async fn install<P>(
    state: &AppState,
    id: i64,
    cancel: &CancellationToken,
    mut report: P,
) -> AppResult<String>
where
    P: FnMut(u64, Option<u64>, String) + Send,
{
    let row = crate::instance::get(&state.db, id).await?;
    if !supported(row.server_type) {
        return Err(AppError::Other(format!(
            "squaremap does not run on a {} server.",
            row.server_type.label()
        )));
    }

    let loader = crate::mods::loader_of(row.server_type, &row.name)?;
    let index = crate::providers::index::ensure_fresh(&state.db, &state.http).await?;
    let source = crate::mods::AnySource::build(state, SourceId::Modrinth).await?;

    report(0, None, "Looking up squaremap".to_string());

    let versions = source
        .versions(
            SLUG,
            &VersionFilter {
                loaders: loader.accepted().iter().map(|l| l.to_string()).collect(),
                game_versions: vec![row.mc_version.clone()],
            },
        )
        .await?;

    let version = crate::mods::resolve::pick_version(&versions, loader, &row.mc_version, &index)
        .ok_or_else(|| {
            AppError::Other(format!(
                "squaremap has no build for {} on Minecraft {}. It can be installed by hand from \
                 the Mods tab if a build exists that this app cannot see.",
                loader.as_str(),
                row.mc_version
            ))
        })?;

    let installed = crate::mods::installed(state, id).await?;
    let plan =
        crate::mods::resolve::plan(&source, version, loader, &row.mc_version, &index, &installed)
            .await?;

    let total = plan.install.len() as u64;
    for (done, planned) in plan.install.iter().enumerate() {
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        report(
            done as u64,
            Some(total),
            format!("Downloading {}", planned.project_title),
        );
        let version = source.version(&planned.version_id).await?;
        crate::mods::install_planned(state, id, planned, &version, cancel).await?;
    }

    remember(state, id).await?;

    // The config is written *before* the first start, which is the only way the
    // chosen port is the one squaremap opens on: it writes its own config on
    // that start, and a port written afterwards takes effect only on the next
    // one. squaremap creates the file only where none exists, so this settles
    // it without ever overwriting somebody's settings.
    let port = free_port(state, id).await?;
    config::ensure_config(&row, port).await?;

    Ok(format!("squaremap installed, on port {port}."))
}

/// Records that this instance is meant to have a map.
pub async fn remember(state: &AppState, id: i64) -> AppResult<()> {
    sqlx::query("UPDATE instances SET map_kind = ?, updated_at = ? WHERE id = ?")
        .bind(STORED)
        .bind(crate::db::now_rfc3339())
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(())
}

/// Forgets it again, after an uninstall.
pub async fn forget(state: &AppState, id: i64) -> AppResult<()> {
    sqlx::query("UPDATE instances SET map_kind = NULL, updated_at = ? WHERE id = ?")
        .bind(crate::db::now_rfc3339())
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(())
}

/// Whether this instance was created asking for a map.
pub fn wanted(instance: &Instance) -> bool {
    instance.map_kind.as_deref() == Some(STORED)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance_at(dir: &Path, server_type: ServerType) -> Instance {
        let mut instance = crate::db::models::Instance::fixture();
        instance.server_type = server_type;
        instance.path = dir.to_string_lossy().to_string();
        instance
    }

    #[test]
    fn it_is_offered_only_where_it_runs() {
        for server_type in [
            ServerType::Fabric,
            ServerType::NeoForge,
            ServerType::Paper,
            ServerType::Purpur,
        ] {
            assert!(supported(server_type), "{server_type:?}");
        }
        // Vanilla loads no mods; squaremap's last Forge build is 1.2.0, for
        // 1.20.1 alone, so Forge would mostly be offered a failure.
        assert!(!supported(ServerType::Vanilla));
        assert!(!supported(ServerType::Forge));
    }

    #[test]
    fn the_jar_as_it_is_really_published_is_recognised() {
        // The exact file from a live install. A hand-written list of names is
        // what made squaremap invisible to the whole feature once already.
        let dir = tempfile::tempdir().unwrap();
        let instance = instance_at(dir.path(), ServerType::Fabric);
        let mods = dir.path().join("mods");
        std::fs::create_dir_all(&mods).unwrap();
        std::fs::write(mods.join("squaremap-fabric-mc26.2-1.3.15.jar"), b"jar").unwrap();

        let found = detect(&instance).unwrap().expect("the jar is found");
        assert_eq!(found.file_name, "squaremap-fabric-mc26.2-1.3.15.jar");
    }

    #[test]
    fn a_disabled_map_still_counts_as_installed() {
        let dir = tempfile::tempdir().unwrap();
        let instance = instance_at(dir.path(), ServerType::Fabric);
        let mods = dir.path().join("mods");
        std::fs::create_dir_all(&mods).unwrap();
        std::fs::write(mods.join("squaremap-fabric-mc26.2-1.3.15.jar.disabled"), b"x").unwrap();

        assert!(detect(&instance).unwrap().is_some());
    }

    #[test]
    fn nothing_else_in_a_mods_folder_is_mistaken_for_it() {
        let dir = tempfile::tempdir().unwrap();
        let instance = instance_at(dir.path(), ServerType::Fabric);
        let mods = dir.path().join("mods");
        std::fs::create_dir_all(&mods).unwrap();
        for file in [
            "fabric-api-0.158.0+26.2.jar",
            "lithium-fabric-0.14.jar",
            "notes.txt",
        ] {
            std::fs::write(mods.join(file), b"x").unwrap();
        }

        assert_eq!(detect(&instance).unwrap(), None);
    }

    #[test]
    fn the_view_opens_where_the_world_is() {
        // A map centred on 0,0 is centred on nothing in particular when the
        // spawn is 3000 blocks away.
        let url = view_url(8081, Some((3000, -1200)));
        assert!(url.starts_with("http://127.0.0.1:8081/?world=minecraft_overworld"), "{url}");
        assert!(url.contains("x=3000"), "{url}");
        assert!(url.contains("z=-1200"), "{url}");

        // No level.dat yet: the plain address, not a guessed position.
        assert_eq!(view_url(8081, None), "http://127.0.0.1:8081");
    }

    #[test]
    fn the_render_command_names_the_world_by_dimension() {
        // Its command parser takes a world identifier; `level-name` is not one.
        assert_eq!(render_command(), "squaremap fullrender minecraft:overworld");
        // Sent from the console, so no leading slash.
        assert!(!render_command().starts_with('/'));
    }

    #[test]
    fn an_unrendered_map_is_recognised_and_a_rendered_one_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // Nothing at all: the state right after installing.
        assert!(barely_rendered(root, ServerType::Fabric));

        let tiles = config::tile_dir(root, ServerType::Fabric).join("minecraft_overworld");
        std::fs::create_dir_all(&tiles).unwrap();
        assert!(barely_rendered(root, ServerType::Fabric), "empty folder");

        for index in 0..(RENDERED_ENOUGH - 1) {
            std::fs::write(tiles.join(format!("{index}.png")), b"tile").unwrap();
        }
        assert!(barely_rendered(root, ServerType::Fabric), "a few tiles");

        std::fs::write(tiles.join("enough.png"), b"tile").unwrap();
        assert!(!barely_rendered(root, ServerType::Fabric));
    }

    #[test]
    fn what_is_stored_is_what_is_read_back() {
        let mut instance = crate::db::models::Instance::fixture();
        assert!(!wanted(&instance));
        instance.map_kind = Some(STORED.to_string());
        assert!(wanted(&instance));
    }
}
