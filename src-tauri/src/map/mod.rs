//! Web maps: BlueMap and Dynmap, installed as mods rather than rendered here.
//!
//! Both projects have spent years on chunk rendering, browser tiles and marker
//! APIs. Reimplementing any of that would produce a worse map that this app then
//! has to maintain, so the job here is smaller and more useful: install the
//! right one for the server type, find out which port it ended up on, and put
//! the address next to the game's.
//!
//! The port is *read*, never assumed. Both mods write a config the user is free
//! to edit, and a map tab pointing at the port we hoped for is worse than no map
//! tab at all.

pub mod config;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use tokio_util::sync::CancellationToken;

use std::path::{Path, PathBuf};

use crate::db::models::{Instance, ServerType};
use crate::error::{AppError, AppResult};
use crate::mods::source::{ModSource, SourceId, VersionFilter};
use crate::state::AppState;

/// The two map mods this app knows how to install and read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub enum MapKind {
    BlueMap,
    Dynmap,
}

impl MapKind {
    /// The Modrinth project, so installation goes through the existing source
    /// path rather than a second downloader.
    pub fn project_slug(self) -> &'static str {
        match self {
            MapKind::BlueMap => "bluemap",
            MapKind::Dynmap => "dynmap",
        }
    }

    /// The text stored in `instances.map_kind`, matching the serde name.
    pub fn as_str(self) -> &'static str {
        match self {
            MapKind::BlueMap => "blue_map",
            MapKind::Dynmap => "dynmap",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MapKind::BlueMap => "BlueMap",
            MapKind::Dynmap => "Dynmap",
        }
    }

    /// The port each project ships with, used only as the starting point for
    /// finding a free one. What a running server uses is read from its config.
    pub fn default_port(self) -> u16 {
        match self {
            MapKind::BlueMap => 8100,
            MapKind::Dynmap => 8123,
        }
    }

    /// The console command that renders a world that has already been played.
    ///
    /// Both maps render as chunks are loaded and saved, so a world played
    /// before the map existed stays blank until it is asked. Sent from the
    /// console, so no leading slash.
    pub fn render_command(self, world: &str) -> String {
        match self {
            MapKind::BlueMap => format!("bluemap update {world}"),
            MapKind::Dynmap => format!("dynmap fullrender {world}"),
        }
    }

    /// How a jar of this project is named, lowercased, as a prefix.
    fn jar_prefix(self) -> &'static str {
        match self {
            MapKind::BlueMap => "bluemap",
            MapKind::Dynmap => "dynmap",
        }
    }

    /// Whether this map runs on that kind of server.
    ///
    /// BlueMap ships a Paper plugin and a mod for each loader. Dynmap has no
    /// Fabric or NeoForge build that this app would install, so offering it for
    /// those would be offering a failure.
    pub fn supports(self, server_type: ServerType) -> bool {
        match self {
            MapKind::BlueMap => matches!(
                server_type,
                ServerType::Fabric
                    | ServerType::Forge
                    | ServerType::NeoForge
                    | ServerType::Paper
                    | ServerType::Purpur
            ),
            MapKind::Dynmap => matches!(
                server_type,
                ServerType::Paper | ServerType::Purpur | ServerType::Forge
            ),
        }
    }
}

/// The maps offered for a server type, best first.
///
/// BlueMap leads where both work: it renders in 3D in the browser and needs no
/// configuration to be useful. Vanilla loads neither, and gets an empty list
/// rather than an offer it cannot honour.
pub fn kinds_for(server_type: ServerType) -> Vec<MapKind> {
    [MapKind::BlueMap, MapKind::Dynmap]
        .into_iter()
        .filter(|kind| kind.supports(server_type))
        .collect()
}

/// The map this app would install if asked for "a web map" with no preference.
pub fn default_for(server_type: ServerType) -> Option<MapKind> {
    kinds_for(server_type).into_iter().next()
}

/// A map mod found in an instance folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub kind: MapKind,
    /// The jar, so the UI can say what was found.
    pub file_name: String,
}

/// Which map mod, if any, is in this instance's content folder.
///
/// The jar name is the evidence: both projects name their file after
/// themselves, and a config folder alone can survive an uninstall.
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
        // `.jar.disabled` is a mod the user switched off; it is still installed.
        if !(lower.ends_with(".jar") || lower.ends_with(".jar.disabled")) {
            continue;
        }
        for kind in [MapKind::BlueMap, MapKind::Dynmap] {
            if lower.starts_with(kind.jar_prefix()) {
                return Ok(Some(Installed {
                    kind,
                    file_name: name,
                }));
            }
        }
    }

    Ok(None)
}

/// A map's port, and who else wants it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct MapStatus {
    /// The map mod installed in this instance, if any.
    pub kind: Option<MapKind>,
    /// The maps this server type could have, for the offer when it has none.
    pub available: Vec<MapKind>,
    /// The port its config names. `None` until the mod has written one, which
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
    /// True when BlueMap's config says it may not download its resources, which
    /// is the state it stops in with "BlueMap is missing important resources!".
    pub download_blocked: bool,
    /// True when almost nothing has been rendered yet. A new map is a black
    /// rectangle, which reads as broken rather than as empty.
    pub barely_rendered: bool,
    /// The world the server loads, for the render command and the hint.
    pub world: Option<String>,
    /// The console command that renders what has already been played, when the
    /// map has one. Built here so the page never assembles a command.
    pub render_command: Option<String>,
}

/// Everything the Map tab needs, read fresh from disk.
pub async fn status(state: &AppState, id: i64) -> AppResult<MapStatus> {
    let row = crate::instance::get(&state.db, id).await?;
    let installed = detect(&row)?;
    let kind = installed.as_ref().map(|found| found.kind);

    let port = match kind {
        Some(kind) => config::read_port(&row, kind).await?,
        None => None,
    };

    let conflict = match port {
        Some(port) => conflict_for(state, id, port).await?,
        None => None,
    };

    // The world the server will load, which is both what the map is named
    // after and what a render command has to name.
    let world = {
        let path = row.path_buf();
        tokio::task::spawn_blocking(move || {
            let properties =
                std::fs::read_to_string(crate::paths::server_properties_path(&path)).ok();
            properties
                .as_deref()
                .and_then(|text| crate::process::port::read_property(text, "level-name"))
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
            found
                .spawn_x
                .zip(found.spawn_z)
                .map(|(x, z)| (x, found.spawn_y.unwrap_or(64), z))
        })
        .await
        .unwrap_or(None)
    };

    let barely_rendered = match kind {
        Some(kind) => {
            let path = row.path_buf();
            let server_type = row.server_type;
            tokio::task::spawn_blocking(move || barely_rendered(&path, server_type, kind))
                .await
                .unwrap_or(false)
        }
        None => false,
    };

    Ok(MapStatus {
        download_blocked: match kind {
            Some(MapKind::BlueMap) => config::download_blocked(&row).await?,
            _ => false,
        },
        barely_rendered,
        render_command: kind.map(|kind| kind.render_command(&world)),
        world: Some(world.clone()),
        available: kinds_for(row.server_type),
        config_path: kind.map(|kind| config::config_path(&row, kind).to_string_lossy().to_string()),
        // Loopback rather than a name: this is the machine the server runs on,
        // and the Networking tab is where the shareable addresses live.
        url: port
            .zip(kind)
            .map(|(port, kind)| view_url(kind, port, &world, spawn)),
        running: state.status_of(&row.uuid).is_live(),
        kind,
        port,
        conflict,
    })
}

/// The address to open, centred on the world's spawn.
///
/// A map opened at 0,0 is opened at nothing in particular — the world is
/// wherever its spawn is, which may be thousands of blocks away. Each project
/// takes the position differently: BlueMap in the URL fragment it also writes
/// when you move around, Dynmap in query parameters.
///
/// Without a spawn (no level.dat yet, or an unreadable one) the plain address
/// is returned rather than a guessed position.
pub fn view_url(kind: MapKind, port: u16, world: &str, spawn: Option<(i64, i64, i64)>) -> String {
    let base = format!("http://127.0.0.1:{port}");
    let Some((x, y, z)) = spawn else {
        return base;
    };

    match kind {
        // #<map>:<x>:<y>:<z>:<distance>:<yaw>:<pitch>:<roll>:<perspective>
        MapKind::BlueMap => format!("{base}/#{world}:{x}:{y}:{z}:500:0:0:0:perspective"),
        MapKind::Dynmap => {
            format!("{base}/?worldname={world}&mapname=surface&zoom=4&x={x}&y={y}&z={z}")
        }
    }
}

/// How full the map's tile folder has to be before it stops counting as new.
///
/// BlueMap writes one file per rendered tile, so a played-in world has hundreds.
/// A dozen is generous for "nothing has been rendered yet" and cheap to count.
const RENDERED_ENOUGH: usize = 12;

/// Where each map keeps the tiles it has rendered.
fn tile_dir(instance_path: &Path, server_type: ServerType, kind: MapKind) -> PathBuf {
    match kind {
        // BlueMap's default `data` folder, holding one folder per map.
        MapKind::BlueMap => instance_path.join("bluemap").join("web").join("maps"),
        MapKind::Dynmap => match server_type {
            ServerType::Paper | ServerType::Purpur => instance_path
                .join("plugins")
                .join("dynmap")
                .join("web")
                .join("tiles"),
            _ => instance_path.join("dynmap").join("web").join("tiles"),
        },
    }
}

/// Whether the map has rendered so little that it will look broken.
///
/// Counting stops at `RENDERED_ENOUGH`: the question is "is this empty", not
/// "how big is it", and a rendered world holds tens of thousands of files.
fn barely_rendered(instance_path: &Path, server_type: ServerType, kind: MapKind) -> bool {
    let root = tile_dir(instance_path, server_type, kind);
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
/// two maps on 8100, and the failure mode is the same — the second web server
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
        if let Some(found) = detect(&other)? {
            if config::read_port(&other, found.kind).await? == Some(port) {
                return Ok(Some(format!("{} (its {})", other.name, found.kind.label())));
            }
        }
    }
    Ok(None)
}

/// A port no other server on this machine is using, starting from the map's own
/// default and counting up.
///
/// Checked against what is listening as well as against the other instances:
/// the map has to coexist with whatever else this computer runs, and a port
/// that is busy today is a map that silently fails on first start.
pub async fn free_port(state: &AppState, id: i64, kind: MapKind) -> AppResult<u16> {
    let start = kind.default_port();
    for candidate in start..start.saturating_add(50) {
        if conflict_for(state, id, candidate).await?.is_some() {
            continue;
        }
        let taken = tokio::task::spawn_blocking(move || {
            !crate::process::port::port_is_free(candidate)
        })
        .await
        .unwrap_or(false);
        if !taken {
            return Ok(candidate);
        }
    }
    // Fifty in a row taken is not a case worth inventing behaviour for; the
    // default at least produces the mod's own error rather than ours.
    Ok(start)
}

/// Installs a map mod and puts it on a port nothing else wants.
///
/// The install itself is the ordinary mod path — same source, same resolver,
/// same allowlisted download host — because a map is a mod. What this adds is
/// choosing the project for the server type and, once the config exists,
/// moving it off a port another server already has.
pub async fn install<P>(
    state: &AppState,
    id: i64,
    kind: MapKind,
    cancel: &CancellationToken,
    mut report: P,
) -> AppResult<String>
where
    P: FnMut(u64, Option<u64>, String) + Send,
{
    let row = crate::instance::get(&state.db, id).await?;
    let loader = crate::mods::loader_of(row.server_type, &row.name)?;
    let index = crate::providers::index::ensure_fresh(&state.db, &state.http).await?;
    let source = crate::mods::AnySource::build(state, SourceId::Modrinth).await?;

    report(0, None, format!("Looking up {}", kind.label()));

    let versions = source
        .versions(
            kind.project_slug(),
            &VersionFilter {
                loaders: loader.accepted().iter().map(|l| l.to_string()).collect(),
                game_versions: vec![row.mc_version.clone()],
            },
        )
        .await?;

    let version = crate::mods::resolve::pick_version(&versions, loader, &row.mc_version, &index)
        .ok_or_else(|| {
            AppError::Other(format!(
                "{} has no build for {} on Minecraft {}. The map can be installed by hand from \
                 the Mods tab if a build exists that this app cannot see.",
                kind.label(),
                loader.as_str(),
                row.mc_version
            ))
        })?;

    let installed = crate::mods::installed(state, id).await?;
    let plan = crate::mods::resolve::plan(&source, version, loader, &row.mc_version, &index, &installed)
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

    remember(state, id, kind).await?;

    let port = free_port(state, id, kind).await?;

    // BlueMap needs its config before its first start, not after it. Two
    // reasons: `accept-download` defaults to false and BlueMap stops on the
    // first start without it ("BlueMap is missing important resources!"), and a
    // port chosen after that start would only take effect on the next one.
    // BlueMap creates these files only when they are missing, so writing them
    // first settles both — and neither is ever overwritten.
    if kind == MapKind::BlueMap {
        config::ensure_core_conf(&row).await?;
        config::ensure_webserver_conf(&row, port).await?;
    } else {
        // Dynmap has no such gate; its config is written on the first start and
        // the port is moved from there.
        let _ = config::write_port(&row, kind, port).await;
    }

    Ok(format!(
        "{} installed, on port {port}{}",
        kind.label(),
        match kind {
            MapKind::BlueMap =>
                ". It downloads a Minecraft client jar from Mojang on the first start, to take \
                 block textures out of it.",
            MapKind::Dynmap => ".",
        }
    ))
}

/// Records which map an instance is meant to have.
pub async fn remember(state: &AppState, id: i64, kind: MapKind) -> AppResult<()> {
    sqlx::query("UPDATE instances SET map_kind = ?, updated_at = ? WHERE id = ?")
        .bind(kind.as_str())
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

/// The map an instance was created wanting, if any.
pub fn wanted(instance: &Instance) -> Option<MapKind> {
    match instance.map_kind.as_deref()? {
        "blue_map" => Some(MapKind::BlueMap),
        "dynmap" => Some(MapKind::Dynmap),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_map_is_offered_only_where_it_runs() {
        // Dynmap has no Fabric or NeoForge build this app would install.
        assert_eq!(
            kinds_for(ServerType::Fabric),
            vec![MapKind::BlueMap],
            "fabric"
        );
        assert_eq!(
            kinds_for(ServerType::NeoForge),
            vec![MapKind::BlueMap],
            "neoforge"
        );
        // Forge and the Bukkit family can have either.
        assert_eq!(
            kinds_for(ServerType::Forge),
            vec![MapKind::BlueMap, MapKind::Dynmap]
        );
        assert_eq!(
            kinds_for(ServerType::Paper),
            vec![MapKind::BlueMap, MapKind::Dynmap]
        );
        assert_eq!(
            kinds_for(ServerType::Purpur),
            vec![MapKind::BlueMap, MapKind::Dynmap]
        );
        // Vanilla loads neither, so nothing is offered rather than something
        // that would fail at install time.
        assert!(kinds_for(ServerType::Vanilla).is_empty());
        assert_eq!(default_for(ServerType::Vanilla), None);
    }

    #[test]
    fn the_default_is_the_one_that_works_everywhere() {
        for server_type in [
            ServerType::Fabric,
            ServerType::Forge,
            ServerType::NeoForge,
            ServerType::Paper,
            ServerType::Purpur,
        ] {
            assert_eq!(default_for(server_type), Some(MapKind::BlueMap));
        }
    }

    #[test]
    fn the_view_opens_where_the_world_is() {
        // A map centred on 0,0 is centred on nothing in particular when the
        // spawn is 3000 blocks away, which is what made a populated map look
        // blank.
        let bluemap = view_url(MapKind::BlueMap, 8100, "world", Some((3000, 71, -1200)));
        assert!(bluemap.starts_with("http://127.0.0.1:8100/#world:3000:71:-1200:"), "{bluemap}");

        let dynmap = view_url(MapKind::Dynmap, 8123, "world", Some((3000, 71, -1200)));
        assert!(dynmap.contains("worldname=world"), "{dynmap}");
        assert!(dynmap.contains("x=3000"), "{dynmap}");
        assert!(dynmap.contains("z=-1200"), "{dynmap}");
    }

    #[test]
    fn a_world_with_no_spawn_yet_gets_the_plain_address() {
        // No level.dat before the first start, and a guessed position would be
        // worse than the map's own default.
        for kind in [MapKind::BlueMap, MapKind::Dynmap] {
            assert_eq!(view_url(kind, 8100, "world", None), "http://127.0.0.1:8100");
        }
    }

    #[test]
    fn the_world_name_travels_into_the_view_and_the_render_command() {
        // A renamed level-name is both the map's id and what a render has to
        // name, so neither may be hardcoded to "world".
        let url = view_url(MapKind::BlueMap, 8100, "survival", Some((0, 64, 0)));
        assert!(url.contains("#survival:"), "{url}");
        assert_eq!(
            MapKind::BlueMap.render_command("survival"),
            "bluemap update survival"
        );
        assert_eq!(
            MapKind::Dynmap.render_command("survival"),
            "dynmap fullrender survival"
        );
        // Console commands, so no leading slash.
        assert!(!MapKind::BlueMap.render_command("survival").starts_with('/'));
    }

    #[test]
    fn an_unrendered_map_is_recognised_and_a_rendered_one_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // Nothing at all: the state right after installing.
        assert!(barely_rendered(root, ServerType::Fabric, MapKind::BlueMap));

        let tiles = root.join("bluemap").join("web").join("maps").join("world").join("tiles");
        std::fs::create_dir_all(&tiles).unwrap();
        assert!(barely_rendered(root, ServerType::Fabric, MapKind::BlueMap), "empty folder");

        // A handful of tiles is still a world nobody has played in.
        for index in 0..(RENDERED_ENOUGH - 1) {
            std::fs::write(tiles.join(format!("{index}.png")), b"tile").unwrap();
        }
        assert!(barely_rendered(root, ServerType::Fabric, MapKind::BlueMap), "a few tiles");

        // Past the threshold it stops claiming the map is empty.
        std::fs::write(tiles.join("enough.png"), b"tile").unwrap();
        assert!(!barely_rendered(root, ServerType::Fabric, MapKind::BlueMap));
    }

    #[test]
    fn dynmap_tiles_follow_the_servers_own_folder_convention() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        let paper = tile_dir(root, ServerType::Paper, MapKind::Dynmap);
        assert!(paper.to_string_lossy().replace('\\', "/").ends_with("plugins/dynmap/web/tiles"));

        let forge = tile_dir(root, ServerType::Forge, MapKind::Dynmap);
        assert!(forge.to_string_lossy().replace('\\', "/").ends_with("dynmap/web/tiles"));
        assert!(!forge.to_string_lossy().contains("plugins"));
    }

    #[test]
    fn the_two_defaults_do_not_collide() {
        assert_ne!(
            MapKind::BlueMap.default_port(),
            MapKind::Dynmap.default_port()
        );
    }
}
