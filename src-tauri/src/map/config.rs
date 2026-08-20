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
    }
}

/// The port from a config file's text, or `None` when it does not say.
///
/// Both formats are `key: value` at heart. Comments start with `#` and, in
/// HOCON, also `//`; a commented-out port is not a port. BlueMap's file has the
/// key inside no block, so a plain line scan is enough — and being strict about
/// the key means `port` never matches BlueMap's neighbouring `accept-download`
/// or Dynmap's `webserver-bindaddress`.
pub fn parse_port(contents: &str, kind: MapKind) -> Option<u16> {
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
