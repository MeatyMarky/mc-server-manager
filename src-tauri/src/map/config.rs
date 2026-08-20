//! Reading and writing the map mods' own port setting.
//!
//! Neither file is a format this app already parses: BlueMap writes HOCON with
//! comments and blocks, Dynmap writes YAML with a hundred keys the user may
//! well have edited. Both are therefore treated the same way `server.properties`
//! is — every line is kept exactly as it was read, and only the one line that
//! changes is rewritten. Anything else would quietly undo somebody's config the
//! first time this app touched it.

use std::path::{Path, PathBuf};

use crate::db::models::{Instance, ServerType};
use crate::error::{AppResult, IoContext};

use super::MapKind;

/// Where each mod keeps the file holding its web port.
///
/// BlueMap uses the same path on every platform it runs on. Dynmap follows the
/// server's own convention: `plugins/` on the Bukkit family, a top-level folder
/// under Forge.
pub fn config_path(instance: &Instance, kind: MapKind) -> PathBuf {
    let root = instance.path_buf();
    match kind {
        MapKind::BlueMap => root.join("config").join("bluemap").join("webserver.conf"),
        MapKind::Squaremap => {
            super::squaremap_data_dir(&root, instance.server_type).join("config.yml")
        }
        MapKind::Dynmap => match instance.server_type {
            ServerType::Paper | ServerType::Purpur => root
                .join("plugins")
                .join("dynmap")
                .join("configuration.txt"),
            _ => root.join("dynmap").join("configuration.txt"),
        },
    }
}

/// The key each file holds the port under.
fn port_key(kind: MapKind) -> &'static str {
    match kind {
        MapKind::BlueMap => "port",
        MapKind::Dynmap => "webserver-port",
        // squaremap nests it: settings.internal-webserver.port. A bare `port`
        // scan would find the wrong key, so this one is read by path.
        MapKind::Squaremap => "port",
    }
}

/// The path squaremap keeps its port under, outermost key first.
const SQUAREMAP_PORT_PATH: [&str; 3] = ["settings", "internal-webserver", "port"];

/// Whether a line at `indent` continues `path`, and the key it holds.
///
/// YAML by indentation, which is all that is needed to tell
/// `settings.internal-webserver.port` from the half-dozen other keys called
/// `port` a config might hold. Only the nesting is tracked — anchors, flow
/// mappings and multi-line strings are not, because squaremap's config uses
/// none of them.
fn nested_key<'a>(line: &'a str, stack: &mut Vec<(usize, String)>) -> Option<(Vec<String>, &'a str)> {
    let body = line.trim_end_matches(['\r', '\n']);
    let trimmed = body.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
        return None;
    }
    let indent = body.len() - trimmed.len();
    let (name, value) = trimmed.split_once(':')?;
    let name = name.trim().trim_matches('"').trim_matches('\'');

    while stack.last().is_some_and(|(depth, _)| *depth >= indent) {
        stack.pop();
    }

    let mut path: Vec<String> = stack.iter().map(|(_, key)| key.clone()).collect();
    path.push(name.to_string());

    if value.trim().is_empty() {
        // A parent: the keys under it are what this was tracking for.
        stack.push((indent, name.to_string()));
    }

    Some((path, value))
}

/// squaremap's config, written before its first start so the chosen port is
/// the one it opens on.
///
/// squaremap fills in every key it does not find, so these three are enough —
/// and unlike BlueMap it has no download to accept: it draws its tiles from the
/// server's own block colours, with nothing to fetch from Mojang.
pub fn squaremap_conf(port: u16) -> String {
    format!(
        "\
# Written by Minecraft Server Manager when squaremap was installed.
# The port was chosen because nothing else on this computer was using it.
settings:
  internal-webserver:
    enabled: true
    bind: 0.0.0.0
    port: {port}
"
    )
}

/// Writes squaremap's config, if it is not there already.
pub async fn ensure_squaremap_conf(instance: &Instance, port: u16) -> AppResult<bool> {
    let path = config_path(instance, MapKind::Squaremap);
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .ctx("create the squaremap config folder", parent)?;
    }
    write_atomic(&path, &squaremap_conf(port)).await?;
    Ok(true)
}

/// BlueMap's second config, holding the one setting it refuses to start without.
pub fn core_config_path(instance: &Instance) -> PathBuf {
    instance
        .path_buf()
        .join("config")
        .join("bluemap")
        .join("core.conf")
}

/// The file BlueMap needs before it will render anything.
///
/// `accept-download: false` is the default, and with it BlueMap stops on the
/// first start with "BlueMap is missing important resources!". Setting it true
/// is a statement on the user's behalf — that they own Minecraft: Java Edition
/// and accept Mojang's EULA — because BlueMap then downloads a Minecraft client
/// jar from Mojang to take block textures out of it. That is why the create
/// dialog's checkbox says so, and why this app writes the file only for a map
/// it was asked to install.
///
/// Written whole rather than edited, because it is written before BlueMap's
/// first start: BlueMap creates the file only when it is missing and fills in
/// every key it does not find, so the three below are enough.
pub const CORE_CONF: &str = "\
## Written by Minecraft Server Manager when BlueMap was installed.
##
## accept-download tells BlueMap it may download a Minecraft client jar from
## Mojang and take the block textures out of it. It was set to true because the
## \"Web map\" box was ticked when this server was created, which says the same
## thing: that you own Minecraft: Java Edition and accept Mojang's EULA.
accept-download: true

## Threads BlueMap renders with. Left at BlueMap's own defaults.
#render-thread-count: 1
";

/// Whether BlueMap has been told it may download its resources.
pub fn accepts_download(contents: &str) -> Option<bool> {
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }
        let Some((name, value)) = trimmed.split_once(':') else {
            continue;
        };
        if name.trim().trim_matches('"') != "accept-download" {
            continue;
        }
        let value = value
            .split(['#', '/'])
            .next()
            .unwrap_or(value)
            .trim()
            .trim_matches('"');
        return match value {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        };
    }
    None
}

/// The port from a config file's text, or `None` when it does not say.
///
/// Both formats are `key: value` at heart. Comments start with `#` and, in
/// HOCON, also `//`; a commented-out port is not a port. BlueMap's file has the
/// key inside no block, so a plain line scan is enough — and being strict about
/// the key means `port` never matches BlueMap's neighbouring `accept-download`
/// or Dynmap's `webserver-bindaddress`.
pub fn parse_port(contents: &str, kind: MapKind) -> Option<u16> {
    if kind == MapKind::Squaremap {
        let mut stack: Vec<(usize, String)> = Vec::new();
        for line in contents.lines() {
            let Some((path, value)) = nested_key(line, &mut stack) else {
                continue;
            };
            if path == SQUAREMAP_PORT_PATH {
                return value
                    .split('#')
                    .next()
                    .unwrap_or(value)
                    .trim()
                    .parse::<u16>()
                    .ok();
            }
        }
        return None;
    }

    let key = port_key(kind);
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }
        let Some((name, value)) = trimmed.split_once(':') else {
            continue;
        };
        // HOCON quotes keys sometimes; YAML does not.
        if name.trim().trim_matches('"') != key {
            continue;
        }
        let value = value
            .split(['#', '/'])
            .next()
            .unwrap_or(value)
            .trim()
            .trim_matches('"');
        if let Ok(port) = value.parse::<u16>() {
            return Some(port);
        }
    }
    None
}

/// The same line, rewritten to a new port, with everything else untouched.
///
/// Returns `None` when the file has no port line to change — writing one in
/// would mean guessing where it belongs in a format this app does not otherwise
/// understand.
pub fn with_port(contents: &str, kind: MapKind, port: u16) -> Option<String> {
    if kind == MapKind::Squaremap {
        let mut stack: Vec<(usize, String)> = Vec::new();
        let mut found = false;
        let mut out = String::with_capacity(contents.len());
        for line in contents.split_inclusive('\n') {
            let body = line.trim_end_matches(['\r', '\n']);
            let ending = &line[body.len()..];
            match nested_key(line, &mut stack) {
                Some((path, _)) if !found && path == SQUAREMAP_PORT_PATH => {
                    let trimmed = body.trim_start();
                    let indent = &body[..body.len() - trimmed.len()];
                    let name = trimmed.split_once(':').map(|(name, _)| name).unwrap_or("port");
                    out.push_str(indent);
                    out.push_str(name);
                    out.push_str(": ");
                    out.push_str(&port.to_string());
                    out.push_str(ending);
                    found = true;
                }
                _ => out.push_str(line),
            }
        }
        return found.then_some(out);
    }

    let key = port_key(kind);
    let mut found = false;
    let mut out = String::with_capacity(contents.len());

    for line in contents.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        let ending = &line[body.len()..];
        let trimmed = body.trim_start();

        if !found && !trimmed.starts_with('#') && !trimmed.starts_with("//") {
            if let Some((name, _)) = trimmed.split_once(':') {
                if name.trim().trim_matches('"') == key {
                    let indent = &body[..body.len() - trimmed.len()];
                    out.push_str(indent);
                    out.push_str(name);
                    out.push_str(": ");
                    out.push_str(&port.to_string());
                    out.push_str(ending);
                    found = true;
                    continue;
                }
            }
        }
        out.push_str(line);
    }

    found.then_some(out)
}

/// The configured port for an installed map, or the project's default when the
/// config has not been written yet.
///
/// A map mod writes its config on its first server start, so "no file" is the
/// normal state between installing and starting, not an error.
pub async fn read_port(instance: &Instance, kind: MapKind) -> AppResult<Option<u16>> {
    let path = config_path(instance, kind);
    match tokio::fs::read_to_string(&path).await {
        Ok(contents) => Ok(parse_port(&contents, kind)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(crate::error::AppError::io("read the map config", &path, err)),
    }
}

/// Writes a port into an existing config, atomically.
///
/// Only ever called for a stopped server: both mods hold their config in memory
/// and rewrite it on shutdown, exactly like `server.properties`.
pub async fn write_port(instance: &Instance, kind: MapKind, port: u16) -> AppResult<bool> {
    let path = config_path(instance, kind);
    let Ok(contents) = tokio::fs::read_to_string(&path).await else {
        return Ok(false);
    };
    let Some(updated) = with_port(&contents, kind, port) else {
        return Ok(false);
    };

    write_atomic(&path, &updated).await?;
    Ok(true)
}

/// BlueMap's webserver config, written whole for the same reason as the core
/// one: the port has to be settled before BlueMap's first start, and the file
/// does not exist until then.
pub fn webserver_conf(port: u16) -> String {
    format!(
        "\
## Written by Minecraft Server Manager when BlueMap was installed.
## The port was chosen because nothing else on this computer was using it.
enabled: true
port: {port}
"
    )
}

/// Writes BlueMap's webserver config, if it is not there already.
pub async fn ensure_webserver_conf(instance: &Instance, port: u16) -> AppResult<bool> {
    let path = config_path(instance, MapKind::BlueMap);
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .ctx("create the BlueMap config folder", parent)?;
    }
    write_atomic(&path, &webserver_conf(port)).await?;
    Ok(true)
}

/// Writes BlueMap's core config, if it is not there already.
///
/// Never overwrites: a file that exists is the user's, including one where they
/// have set `accept-download` back to false. Returns whether it wrote.
pub async fn ensure_core_conf(instance: &Instance) -> AppResult<bool> {
    let path = core_config_path(instance);
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .ctx("create the BlueMap config folder", parent)?;
    }
    write_atomic(&path, CORE_CONF).await?;
    Ok(true)
}

/// Whether BlueMap will refuse to start for want of the download flag.
///
/// Read rather than assumed, like the port: the user may have turned it off on
/// purpose, and this is what turns "it just does not work" into a sentence.
pub async fn download_blocked(instance: &Instance) -> AppResult<bool> {
    let path = core_config_path(instance);
    match tokio::fs::read_to_string(&path).await {
        Ok(contents) => Ok(accepts_download(&contents) == Some(false)),
        Err(_) => Ok(false),
    }
}

/// Turns the download flag on in a config BlueMap already wrote.
///
/// The line-targeted edit again: everything else in the file is the user's.
/// Only ever called from an explicit click, because of what the setting means.
pub async fn accept_download(instance: &Instance) -> AppResult<bool> {
    let path = core_config_path(instance);
    let Ok(contents) = tokio::fs::read_to_string(&path).await else {
        // Nothing written yet, so the whole file can be.
        return ensure_core_conf(instance).await;
    };

    let mut found = false;
    let mut out = String::with_capacity(contents.len());
    for line in contents.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        let ending = &line[body.len()..];
        let trimmed = body.trim_start();
        if !found && !trimmed.starts_with('#') && !trimmed.starts_with("//") {
            if let Some((name, _)) = trimmed.split_once(':') {
                if name.trim().trim_matches('"') == "accept-download" {
                    let indent = &body[..body.len() - trimmed.len()];
                    out.push_str(indent);
                    out.push_str(name);
                    out.push_str(": true");
                    out.push_str(ending);
                    found = true;
                    continue;
                }
            }
        }
        out.push_str(line);
    }

    if !found {
        return Ok(false);
    }
    write_atomic(&path, &out).await?;
    Ok(true)
}

async fn write_atomic(path: &Path, contents: &str) -> AppResult<()> {
    let temp = path.with_extension("msm-tmp");
    tokio::fs::write(&temp, contents)
        .await
        .ctx("write the map config", &temp)?;
    tokio::fs::rename(&temp, path)
        .await
        .ctx("replace the map config", path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLUEMAP: &str = r#"## The webserver configuration
accept-download: true

# The IP the webserver binds to
ip: "0.0.0.0"

# The port the webserver listens on
port: 8100

webroot: "bluemap/web"
"#;

    const DYNMAP: &str = r#"# Dynmap configuration
components:
  - class: org.dynmap.ClientComponent
webserver-bindaddress: 0.0.0.0
webserver-port: 8123
disable-webserver: false
"#;

    #[test]
    fn the_port_is_read_from_each_format() {
        assert_eq!(parse_port(BLUEMAP, MapKind::BlueMap), Some(8100));
        assert_eq!(parse_port(DYNMAP, MapKind::Dynmap), Some(8123));
    }

    #[test]
    fn a_user_who_changed_the_port_is_believed() {
        // The whole reason this is read rather than assumed.
        let edited = BLUEMAP.replace("port: 8100", "port: 9000");
        assert_eq!(parse_port(&edited, MapKind::BlueMap), Some(9000));

        let edited = DYNMAP.replace("webserver-port: 8123", "webserver-port: 25580");
        assert_eq!(parse_port(&edited, MapKind::Dynmap), Some(25580));
    }

    #[test]
    fn a_neighbouring_key_is_never_mistaken_for_the_port() {
        // "webserver-bindaddress" contains no port, and BlueMap's file has
        // several keys that end in something numeric.
        assert_eq!(parse_port("webserver-bindaddress: 0.0.0.0\n", MapKind::Dynmap), None);
        assert_eq!(parse_port("support-port: 1234\n", MapKind::BlueMap), None);
        assert_eq!(parse_port("webport: 1234\n", MapKind::Dynmap), None);
    }

    #[test]
    fn a_commented_out_port_is_not_a_port() {
        assert_eq!(parse_port("# port: 8100\n", MapKind::BlueMap), None);
        assert_eq!(parse_port("// port: 8100\n", MapKind::BlueMap), None);
        assert_eq!(parse_port("#webserver-port: 8123\n", MapKind::Dynmap), None);
        // And the real one below a commented one still wins.
        assert_eq!(
            parse_port("# port: 8100\nport: 9001\n", MapKind::BlueMap),
            Some(9001)
        );
    }

    #[test]
    fn a_trailing_comment_does_not_break_the_number() {
        assert_eq!(
            parse_port("port: 8100 # the default\n", MapKind::BlueMap),
            Some(8100)
        );
    }

    #[test]
    fn rewriting_the_port_leaves_every_other_line_alone() {
        let updated = with_port(BLUEMAP, MapKind::BlueMap, 8200).expect("the key is there");
        assert!(updated.contains("port: 8200"));
        assert!(!updated.contains("port: 8100"));

        // Comments, ordering and unrelated keys survive byte for byte.
        for line in BLUEMAP.lines().filter(|line| !line.contains("port: 8100")) {
            assert!(updated.contains(line), "lost: {line}");
        }
    }

    #[test]
    fn rewriting_keeps_the_files_own_line_endings() {
        let crlf = DYNMAP.replace('\n', "\r\n");
        let updated = with_port(&crlf, MapKind::Dynmap, 8200).unwrap();
        assert!(updated.contains("webserver-port: 8200\r\n"));
        assert_eq!(updated.matches('\n').count(), crlf.matches('\n').count());
        assert!(!updated.contains("\n\n"), "no stray blank lines");
    }

    #[test]
    fn a_file_without_the_key_is_left_alone() {
        // Rather than inventing a line in a format we do not fully parse.
        assert_eq!(with_port("nothing: here\n", MapKind::BlueMap, 8100), None);
    }

    #[tokio::test]
    async fn the_config_this_app_writes_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let mut instance = crate::db::models::Instance::fixture();
        instance.path = dir.path().to_string_lossy().to_string();

        assert!(ensure_core_conf(&instance).await.unwrap(), "written once");
        // A user who turns the download back off keeps that answer.
        let path = core_config_path(&instance);
        std::fs::write(&path, "accept-download: false\n").unwrap();
        assert!(!ensure_core_conf(&instance).await.unwrap(), "not written twice");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "accept-download: false\n"
        );
        assert!(download_blocked(&instance).await.unwrap());
    }

    #[tokio::test]
    async fn allowing_the_download_edits_only_that_line() {
        let dir = tempfile::tempdir().unwrap();
        let mut instance = crate::db::models::Instance::fixture();
        instance.path = dir.path().to_string_lossy().to_string();

        // BlueMap's own file, as it writes it after a refused first start.
        let path = core_config_path(&instance);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "## BlueMap core config\ndata: \"bluemap\"\naccept-download: false\nrender-thread-count: 2\n",
        )
        .unwrap();

        assert!(accept_download(&instance).await.unwrap());
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("accept-download: true"));
        assert!(after.contains("render-thread-count: 2"), "{after}");
        assert!(after.contains("## BlueMap core config"), "{after}");
        assert!(!download_blocked(&instance).await.unwrap());
    }

    /// squaremap's config as it ships it, trimmed to the shape that matters:
    /// the port is nested, and there are other keys called `port` around it.
    const SQUAREMAP: &str = r#"# squaremap config
settings:
  web-address: "http://localhost:8080"
  internal-webserver:
    enabled: true
    bind: 0.0.0.0
    port: 8080
    flush-json-immediately: false
  other-service:
    port: 9999
world-settings:
  default:
    zoom:
      default: 3
"#;

    #[test]
    fn squaremaps_nested_port_is_read_by_its_path() {
        assert_eq!(parse_port(SQUAREMAP, MapKind::Squaremap), Some(8080));
    }

    #[test]
    fn a_different_key_called_port_is_not_squaremaps() {
        // `other-service.port` sits at the same depth and would fool a flat
        // scan; so would a `port` under world-settings.
        let moved = SQUAREMAP.replace("    port: 8080", "    port: 8085");
        assert_eq!(parse_port(&moved, MapKind::Squaremap), Some(8085));

        let without = SQUAREMAP.replace("    port: 8080\n", "");
        assert_eq!(
            parse_port(&without, MapKind::Squaremap),
            None,
            "other-service.port must not be mistaken for it"
        );
    }

    #[test]
    fn rewriting_squaremaps_port_leaves_its_neighbours_alone() {
        let updated = with_port(SQUAREMAP, MapKind::Squaremap, 8090).expect("the key is there");
        assert_eq!(parse_port(&updated, MapKind::Squaremap), Some(8090));
        // The other port, the indentation and every comment survive.
        assert!(updated.contains("  other-service:\n    port: 9999"), "{updated}");
        assert!(updated.contains("    port: 8090"), "{updated}");
        assert!(updated.contains("# squaremap config"));
        assert_eq!(updated.lines().count(), SQUAREMAP.lines().count());
    }

    #[test]
    fn a_commented_out_squaremap_port_is_not_a_port() {
        let commented = SQUAREMAP.replace("    port: 8080", "    #port: 8080");
        assert_eq!(parse_port(&commented, MapKind::Squaremap), None);
    }

    #[test]
    fn the_written_squaremap_config_is_read_back_by_the_same_parser() {
        let written = squaremap_conf(8081);
        assert_eq!(parse_port(&written, MapKind::Squaremap), Some(8081));
        assert!(written.contains("enabled: true"));
    }

    #[tokio::test]
    async fn squaremaps_config_is_written_once_and_never_over() {
        let dir = tempfile::tempdir().unwrap();
        let mut instance = crate::db::models::Instance::fixture();
        instance.server_type = ServerType::Fabric;
        instance.path = dir.path().to_string_lossy().to_string();

        assert!(ensure_squaremap_conf(&instance, 8080).await.unwrap());
        assert_eq!(read_port(&instance, MapKind::Squaremap).await.unwrap(), Some(8080));

        // A second install, or a user who moved the port, keeps their file.
        let path = config_path(&instance, MapKind::Squaremap);
        std::fs::write(&path, "settings:\n  internal-webserver:\n    port: 9001\n").unwrap();
        assert!(!ensure_squaremap_conf(&instance, 8080).await.unwrap());
        assert_eq!(read_port(&instance, MapKind::Squaremap).await.unwrap(), Some(9001));
    }

    #[test]
    fn the_written_webserver_config_carries_the_chosen_port() {
        let written = webserver_conf(8104);
        assert_eq!(parse_port(&written, MapKind::BlueMap), Some(8104));
        // And the file this app writes can be read back by the same parser it
        // uses on BlueMap's own, which is the point of writing it at all.
        assert!(written.contains("enabled: true"));
    }

    #[test]
    fn the_written_core_config_accepts_the_download_and_says_why() {
        assert_eq!(accepts_download(CORE_CONF), Some(true));
        // The file explains itself where somebody will find it: in the file.
        assert!(CORE_CONF.contains("Mojang"));
        assert!(CORE_CONF.contains("Web map"));
    }

    #[test]
    fn a_refusal_to_download_is_read_back_as_one() {
        assert_eq!(accepts_download("accept-download: false\n"), Some(false));
        assert_eq!(accepts_download("accept-download: true\n"), Some(true));
        // Commented out is not a setting, and neither is a neighbouring key.
        assert_eq!(accepts_download("# accept-download: true\n"), None);
        assert_eq!(accepts_download("accept-downloads: true\n"), None);
        assert_eq!(accepts_download("data: \"bluemap\"\n"), None);
    }

    #[test]
    fn dynmap_follows_the_servers_own_folder_convention() {
        let mut instance = crate::db::models::Instance::fixture();
        instance.path = "Z:/survival".into();

        instance.server_type = ServerType::Paper;
        assert!(config_path(&instance, MapKind::Dynmap)
            .to_string_lossy()
            .replace('\\', "/")
            .ends_with("plugins/dynmap/configuration.txt"));

        instance.server_type = ServerType::Forge;
        assert!(config_path(&instance, MapKind::Dynmap)
            .to_string_lossy()
            .replace('\\', "/")
            .ends_with("survival/dynmap/configuration.txt"));

        // BlueMap is the same everywhere it runs.
        assert!(config_path(&instance, MapKind::BlueMap)
            .to_string_lossy()
            .replace('\\', "/")
            .ends_with("config/bluemap/webserver.conf"));
    }
}
