//! Reading and writing squaremap's own settings.
//!
//! Its config is YAML with a hundred keys somebody may well have edited, so it
//! is treated the way `server.properties` is: every line is kept exactly as it
//! was read, and only the one line that changes is rewritten.
//!
//! The port is nested — `settings.internal-webserver.port` — and a config holds
//! other keys called `port`, so it is found by path rather than by a flat scan
//! for the name.

use std::path::{Path, PathBuf};

use crate::db::models::{Instance, ServerType};
use crate::error::{AppResult, IoContext};

/// squaremap's data folder: `plugins/squaremap` on the Bukkit family, and a
/// top-level `squaremap` beside the server jar on the mod loaders.
///
/// Confirmed against a real Fabric install rather than assumed: the folder that
/// appeared was `<instance>/squaremap/`, holding `config.yml`, `advanced.yml`,
/// `data/` and `locale/`.
pub fn data_dir(instance_path: &Path, server_type: ServerType) -> PathBuf {
    match server_type {
        ServerType::Paper | ServerType::Purpur => instance_path.join("plugins").join("squaremap"),
        _ => instance_path.join("squaremap"),
    }
}

/// The file holding the port and the bind address.
pub fn config_path(instance: &Instance) -> PathBuf {
    data_dir(&instance.path_buf(), instance.server_type).join("config.yml")
}

/// Where the rendered tiles land, for the "nothing here yet" check.
pub fn tile_dir(instance_path: &Path, server_type: ServerType) -> PathBuf {
    data_dir(instance_path, server_type).join("web").join("tiles")
}

/// The port, outermost key first.
const PORT_PATH: [&str; 3] = ["settings", "internal-webserver", "port"];
/// And the address it binds to, one key along.
const BIND_PATH: [&str; 3] = ["settings", "internal-webserver", "bind"];

/// The key path a line sits at, and the value it holds.
///
/// YAML by indentation, which is all that is needed to tell
/// `settings.internal-webserver.port` from the other keys called `port` in the
/// same file. Anchors, flow mappings and multi-line strings are not tracked,
/// because squaremap's config uses none of them.
fn nested_key<'a>(
    line: &'a str,
    stack: &mut Vec<(usize, String)>,
) -> Option<(Vec<String>, &'a str)> {
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

/// The value at a key path, as written.
fn value_at(contents: &str, path: &[&str]) -> Option<String> {
    let mut stack: Vec<(usize, String)> = Vec::new();
    for line in contents.lines() {
        let Some((here, value)) = nested_key(line, &mut stack) else {
            continue;
        };
        if here == path {
            let value = value.split('#').next().unwrap_or(value).trim();
            return (!value.is_empty()).then(|| value.trim_matches('"').to_string());
        }
    }
    None
}

/// The port squaremap's config names, or `None` when it has not written one.
///
/// The config does not exist until the server's first start, so "no file" is a
/// normal state rather than an error.
pub fn parse_port(contents: &str) -> Option<u16> {
    value_at(contents, &PORT_PATH)?.parse().ok()
}

/// The address its web server binds to.
///
/// Worth reading rather than assuming: squaremap ships `0.0.0.0`, so its map is
/// on the network from the first start.
pub fn parse_bind(contents: &str) -> Option<String> {
    value_at(contents, &BIND_PATH)
}

/// Whether a bind address lets anything but this computer connect.
pub fn reaches_the_network(bind: &str) -> bool {
    !matches!(bind.trim(), "127.0.0.1" | "localhost" | "::1")
}

/// The same file with the port changed, and nothing else touched.
///
/// `None` when the file has no port line to change: writing one in would mean
/// guessing where it belongs in a file this app does not otherwise understand.
pub fn with_port(contents: &str, port: u16) -> Option<String> {
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut found = false;
    let mut out = String::with_capacity(contents.len());

    for line in contents.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        let ending = &line[body.len()..];
        match nested_key(line, &mut stack) {
            Some((path, _)) if !found && path == PORT_PATH => {
                let trimmed = body.trim_start();
                let indent = &body[..body.len() - trimmed.len()];
                let name = trimmed
                    .split_once(':')
                    .map(|(name, _)| name)
                    .unwrap_or("port");
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

    found.then_some(out)
}

/// The config this app writes before squaremap's first start.
///
/// Written whole, because the port has to be settled before that start: the
/// file does not exist until then, and a port written afterwards only takes
/// effect on the *next* start. squaremap fills in every key it does not find,
/// and never overwrites a config that already exists.
pub fn starter_config(port: u16) -> String {
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

/// The port squaremap's config on disk names, when it has one.
pub async fn read_port(instance: &Instance) -> AppResult<Option<u16>> {
    let path = config_path(instance);
    match tokio::fs::read_to_string(&path).await {
        Ok(contents) => Ok(parse_port(&contents)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(crate::error::AppError::io("read the map config", &path, err)),
    }
}

/// The bind address on disk, when there is one.
pub async fn read_bind(instance: &Instance) -> AppResult<Option<String>> {
    match tokio::fs::read_to_string(config_path(instance)).await {
        Ok(contents) => Ok(parse_bind(&contents)),
        Err(_) => Ok(None),
    }
}

/// Writes the starter config, if squaremap has not written one already.
///
/// Never overwrites: a file that exists is the user's, including one where they
/// moved the port themselves. Returns whether it wrote.
pub async fn ensure_config(instance: &Instance, port: u16) -> AppResult<bool> {
    let path = config_path(instance);
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .ctx("create the squaremap config folder", parent)?;
    }
    write_atomic(&path, &starter_config(port)).await?;
    Ok(true)
}

/// Puts a port into an existing config, atomically.
///
/// Only ever called for a stopped server: squaremap holds its config in memory
/// and rewrites it on shutdown, exactly like `server.properties`.
pub async fn write_port(instance: &Instance, port: u16) -> AppResult<bool> {
    let path = config_path(instance);
    let Ok(contents) = tokio::fs::read_to_string(&path).await else {
        return Ok(false);
    };
    let Some(updated) = with_port(&contents, port) else {
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

    /// squaremap's own config, as it wrote it on a real Fabric server: four
    /// spaces of indentation, the port nested three deep, and other keys called
    /// `port` around it.
    const REAL: &str = r#"settings:
    internal-webserver:
        enabled: true
        bind: 0.0.0.0
        port: 8080
        flush-json-immediately: false
    language-file: lang-en.yml
    web-address: http://localhost:8080
    other-service:
        port: 9999
config-version: 2
world-settings:
    default:
        zoom:
            default: 3
"#;

    #[test]
    fn the_port_is_found_by_path_not_by_name() {
        assert_eq!(parse_port(REAL), Some(8080));
        // The decoy at the same depth is not it.
        let without = REAL.replace("        port: 8080\n", "");
        assert_eq!(parse_port(&without), None);
    }

    #[test]
    fn a_user_who_moved_the_port_is_believed() {
        let moved = REAL.replace("port: 8080", "port: 8085");
        assert_eq!(parse_port(&moved), Some(8085));
    }

    #[test]
    fn a_commented_out_port_is_not_a_port() {
        let commented = REAL.replace("        port: 8080", "        #port: 8080");
        assert_eq!(parse_port(&commented), None);
    }

    #[test]
    fn the_bind_address_says_who_can_reach_the_map() {
        // squaremap ships wide open, which is the thing worth knowing.
        assert_eq!(parse_bind(REAL).as_deref(), Some("0.0.0.0"));
        assert!(reaches_the_network("0.0.0.0"));
        assert!(reaches_the_network("192.168.1.24"));
        assert!(!reaches_the_network("127.0.0.1"));
        assert!(!reaches_the_network("localhost"));
        assert!(!reaches_the_network("::1"));
    }

    #[test]
    fn rewriting_the_port_leaves_every_other_line_alone() {
        let updated = with_port(REAL, 8090).expect("the key is there");
        assert_eq!(parse_port(&updated), Some(8090));
        assert!(
            updated.contains("    other-service:\n        port: 9999"),
            "{updated}"
        );
        assert!(updated.contains("config-version: 2"));
        assert_eq!(updated.lines().count(), REAL.lines().count());
    }

    #[test]
    fn rewriting_keeps_the_files_own_line_endings() {
        let crlf = REAL.replace('\n', "\r\n");
        let updated = with_port(&crlf, 8090).unwrap();
        assert!(updated.contains("port: 8090\r\n"));
        assert_eq!(updated.matches('\n').count(), crlf.matches('\n').count());
    }

    #[test]
    fn a_file_without_the_key_is_left_alone() {
        assert_eq!(with_port("nothing: here\n", 8080), None);
    }

    #[test]
    fn the_config_this_app_writes_is_read_back_by_its_own_parser() {
        let written = starter_config(8081);
        assert_eq!(parse_port(&written), Some(8081));
        assert_eq!(parse_bind(&written).as_deref(), Some("0.0.0.0"));
    }

    #[test]
    fn the_data_folder_follows_the_servers_own_convention() {
        let root = Path::new("Z:/survival");
        for server_type in [ServerType::Paper, ServerType::Purpur] {
            let dir = data_dir(root, server_type);
            assert!(dir
                .to_string_lossy()
                .replace('\\', "/")
                .ends_with("plugins/squaremap"));
        }
        // Confirmed against a real Fabric install: a top-level folder.
        for server_type in [ServerType::Fabric, ServerType::NeoForge] {
            let dir = data_dir(root, server_type);
            assert!(dir
                .to_string_lossy()
                .replace('\\', "/")
                .ends_with("survival/squaremap"));
            assert!(!dir.to_string_lossy().contains("plugins"));
        }
    }

    #[tokio::test]
    async fn the_config_is_written_once_and_never_over() {
        let dir = tempfile::tempdir().unwrap();
        let mut instance = crate::db::models::Instance::fixture();
        instance.server_type = ServerType::Fabric;
        instance.path = dir.path().to_string_lossy().to_string();

        assert!(ensure_config(&instance, 8081).await.unwrap());
        assert_eq!(read_port(&instance).await.unwrap(), Some(8081));

        // A second install, or a user who moved the port, keeps their file.
        std::fs::write(
            config_path(&instance),
            "settings:\n  internal-webserver:\n    port: 9001\n",
        )
        .unwrap();
        assert!(!ensure_config(&instance, 8081).await.unwrap());
        assert_eq!(read_port(&instance).await.unwrap(), Some(9001));

        // And moving it afterwards edits that same file.
        assert!(write_port(&instance, 8090).await.unwrap());
        assert_eq!(read_port(&instance).await.unwrap(), Some(8090));
    }
}
