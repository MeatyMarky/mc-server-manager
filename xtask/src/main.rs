//! Repository maintenance tasks.
//!
//! ```text
//! cargo xtask refresh-fixtures    # re-record the provider API fixtures
//! ```
//!
//! Provider tests run against recorded payloads so CI never depends on Mojang,
//! PaperMC or Forge being up. Those recordings go stale as the APIs publish new
//! versions — that is expected maintenance, not a break. This task re-records
//! them in one step, trimming each payload to the handful of entries the tests
//! actually assert on.

use std::error::Error;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const USER_AGENT: &str = "mc-server-manager/0.1 (fixture refresh)";

/// Versions kept in the trimmed fixtures: one per era, plus the oldest entries
/// the parsers are expected to cope with.
const KEEP_VERSIONS: &[&str] = &[
    "26.3-snapshot-9",
    "26.2",
    "26.1.2",
    "1.21.4",
    "1.20.4",
    "1.16.5",
    "1.12.2",
];

fn main() {
    let task = std::env::args().nth(1).unwrap_or_default();
    let result = match task.as_str() {
        "refresh-fixtures" => refresh_fixtures(),
        "" | "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        other => {
            eprintln!("unknown task: {other}\n");
            print_help();
            std::process::exit(2);
        }
    };

    if let Err(err) = result {
        eprintln!("xtask failed: {err}");
        std::process::exit(1);
    }
}

fn print_help() {
    println!("Tasks:");
    println!("  refresh-fixtures    re-record src-tauri/tests/fixtures from the live APIs");
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives next to src-tauri")
        .join("src-tauri")
        .join("tests")
        .join("fixtures")
}

fn client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(60))
        .build()?)
}

fn get_text(client: &reqwest::blocking::Client, url: &str) -> Result<String> {
    let response = client.get(url).send()?;
    if !response.status().is_success() {
        return Err(format!("{url} answered {}", response.status()).into());
    }
    Ok(response.text()?)
}

fn get_json(client: &reqwest::blocking::Client, url: &str) -> Result<Value> {
    Ok(serde_json::from_str(&get_text(client, url)?)?)
}

fn write(name: &str, contents: &str) -> Result<()> {
    let path = fixtures_dir().join(name);
    // Always LF: the repo normalises line endings and fixtures are compared as text.
    std::fs::write(&path, contents.replace("\r\n", "\n"))?;
    println!("  wrote {name} ({} bytes)", contents.len());
    Ok(())
}

fn write_json(name: &str, value: &Value) -> Result<()> {
    write(name, &format!("{}\n", serde_json::to_string_pretty(value)?))
}

fn refresh_fixtures() -> Result<()> {
    let client = client()?;
    println!("Refreshing fixtures in {}", fixtures_dir().display());

    vanilla(&client)?;
    paper(&client)?;
    purpur(&client)?;
    fabric(&client)?;
    forge(&client)?;
    neoforge(&client)?;
    adoptium(&client)?;

    println!("\nDone. Review the diff, then run:");
    println!("  cargo test");
    println!("  cargo test --test network_smoke -- --ignored --nocapture");
    Ok(())
}

fn vanilla(client: &reqwest::blocking::Client) -> Result<()> {
    println!("Vanilla…");
    let manifest = get_json(
        client,
        "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json",
    )?;

    let versions = manifest["versions"]
        .as_array()
        .ok_or("manifest has no versions array")?;
    let kept: Vec<Value> = versions
        .iter()
        .filter(|v| KEEP_VERSIONS.contains(&v["id"].as_str().unwrap_or_default()))
        .cloned()
        .collect();
    if kept.is_empty() {
        return Err("none of the pinned versions are in the manifest any more".into());
    }
    write_json(
        "vanilla_version_manifest_v2.json",
        &json!({ "latest": manifest["latest"], "versions": kept }),
    )?;

    // Per-version detail, trimmed to what the resolver reads.
    for (id, file) in [
        ("26.2", "vanilla_version_26_2.json"),
        ("1.21.4", "vanilla_version_1_21_4.json"),
        ("1.16.5", "vanilla_version_1_16_5.json"),
    ] {
        let url = kept
            .iter()
            .find(|v| v["id"] == id)
            .and_then(|v| v["url"].as_str())
            .ok_or_else(|| format!("{id} is not in the manifest"))?;
        let detail = get_json(client, url)?;
        write_json(
            file,
            &json!({
                "id": detail["id"],
                "type": detail["type"],
                "javaVersion": detail["javaVersion"],
                "downloads": {
                    "server": detail["downloads"]["server"],
                    "client": {
                        "sha1": detail["downloads"]["client"]["sha1"],
                        "size": detail["downloads"]["client"]["size"],
                        "url": detail["downloads"]["client"]["url"],
                    }
                }
            }),
        )?;
    }
    Ok(())
}

fn paper(client: &reqwest::blocking::Client) -> Result<()> {
    println!("Paper…");
    let project = get_json(client, "https://fill.papermc.io/v3/projects/paper")?;
    let versions = project["versions"]
        .as_object()
        .ok_or("paper project has no versions map")?;
    let kept: serde_json::Map<String, Value> = versions
        .iter()
        .filter(|(family, _)| ["26.2", "26.1", "1.21", "1.20"].contains(&family.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    write_json(
        "paper_project.json",
        &json!({ "project": project["project"], "versions": kept }),
    )?;

    let builds = get_json(
        client,
        "https://fill.papermc.io/v3/projects/paper/versions/1.21.4/builds",
    )?;
    let builds = builds.as_array().ok_or("paper builds is not an array")?;
    let sample: Vec<Value> = [0, builds.len() / 2, builds.len().saturating_sub(1)]
        .iter()
        .filter_map(|index| builds.get(*index).cloned())
        .collect();
    write_json("paper_builds_1_21_4.json", &json!(sample))
}

fn purpur(client: &reqwest::blocking::Client) -> Result<()> {
    println!("Purpur…");
    let root = get_json(client, "https://api.purpurmc.org/v2/purpur")?;
    let all = root["versions"].as_array().ok_or("purpur has no versions")?;
    let tail: Vec<Value> = all.iter().rev().take(8).rev().cloned().collect();
    write_json(
        "purpur_root.json",
        &json!({ "project": root["project"], "metadata": root["metadata"], "versions": tail }),
    )?;

    let version = get_json(client, "https://api.purpurmc.org/v2/purpur/1.21.4")?;
    let builds = version["builds"]["all"]
        .as_array()
        .ok_or("purpur version has no builds")?;
    let latest = version["builds"]["latest"].clone();
    let tail: Vec<Value> = builds.iter().rev().take(5).rev().cloned().collect();
    write_json(
        "purpur_version_1_21_4.json",
        &json!({
            "project": version["project"],
            "version": version["version"],
            "builds": { "latest": latest.clone(), "all": tail }
        }),
    )?;

    let latest = latest.as_str().ok_or("purpur latest build is not a string")?;
    let build = get_json(
        client,
        &format!("https://api.purpurmc.org/v2/purpur/1.21.4/{latest}"),
    )?;
    let mut trimmed = build.clone();
    if let Some(object) = trimmed.as_object_mut() {
        object.remove("commits");
    }
    write_json("purpur_build_1_21_4.json", &trimmed)
}

fn fabric(client: &reqwest::blocking::Client) -> Result<()> {
    println!("Fabric…");
    let game = get_json(client, "https://meta.fabricmc.net/v2/versions/game")?;
    let kept: Vec<Value> = game
        .as_array()
        .ok_or("fabric game versions is not an array")?
        .iter()
        .filter(|v| KEEP_VERSIONS.contains(&v["version"].as_str().unwrap_or_default()))
        .cloned()
        .collect();
    write_json("fabric_game.json", &json!(kept))?;

    for (url, file, take) in [
        ("https://meta.fabricmc.net/v2/versions/loader", "fabric_loader.json", 4),
        (
            "https://meta.fabricmc.net/v2/versions/installer",
            "fabric_installer.json",
            3,
        ),
    ] {
        let all = get_json(client, url)?;
        let head: Vec<Value> = all
            .as_array()
            .ok_or("fabric list is not an array")?
            .iter()
            .take(take)
            .cloned()
            .collect();
        write_json(file, &json!(head))?;
    }
    Ok(())
}

/// The Adoptium responses the managed-runtime resolver is tested against.
///
/// Recorded per platform because the archive extension and the release line
/// differ: a Windows JDK is a .zip, a Linux one a .tar.gz.
fn adoptium(client: &reqwest::blocking::Client) -> Result<()> {
    println!("Adoptium…");
    for (feature, os, arch) in [
        (25, "windows", "x64"),
        (21, "linux", "x64"),
        (17, "windows", "x64"),
        (8, "linux", "aarch64"),
    ] {
        let url = format!(
            "https://api.adoptium.net/v3/assets/latest/{feature}/hotspot             ?os={os}&architecture={arch}&image_type=jdk&vendor=eclipse"
        );
        let body = get_json(client, &url)?;
        write_json(&format!("adoptium_latest_{feature}_{os}_{arch}.json"), &body)?;
    }
    Ok(())
}

fn forge(client: &reqwest::blocking::Client) -> Result<()> {
    println!("Forge…");
    let promotions = get_json(
        client,
        "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json",
    )?;
    let promos = promotions["promos"]
        .as_object()
        .ok_or("forge promotions has no promos")?;
    let kept: serde_json::Map<String, Value> = promos
        .iter()
        .filter(|(key, _)| {
            let version = key.rsplit_once('-').map(|(v, _)| v).unwrap_or_default();
            ["1.12.2", "1.16.5", "1.20.1", "1.21.4", "26.1.2", "26.2"].contains(&version)
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    write_json(
        "forge_promotions_slim.json",
        &json!({ "homepage": promotions["homepage"], "promos": kept }),
    )?;

    let xml = get_text(
        client,
        "https://maven.minecraftforge.net/net/minecraftforge/forge/maven-metadata.xml",
    )?;
    let versions = maven_versions(&xml);
    let mut kept: Vec<String> = versions
        .iter()
        .filter(|v| v.starts_with("1.21.4-"))
        .rev()
        .take(3)
        .cloned()
        .collect();
    kept.extend(
        versions
            .iter()
            .filter(|v| v.starts_with("26.2-"))
            .rev()
            .take(3)
            .cloned(),
    );
    kept.reverse();
    write(
        "forge_maven_metadata.xml",
        &maven_document("net.minecraftforge", "forge", &kept),
    )
}

fn neoforge(client: &reqwest::blocking::Client) -> Result<()> {
    println!("NeoForge…");
    let xml = get_text(
        client,
        "https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml",
    )?;
    let versions = maven_versions(&xml);
    // Both encodings have to stay represented: three-part (classic) and
    // four-part (calendar).
    let mut kept: Vec<String> = versions
        .iter()
        .filter(|v| v.starts_with("20.4.") || v.starts_with("21.1."))
        .take(3)
        .cloned()
        .collect();
    let calendar: Vec<String> = versions
        .iter()
        .filter(|v| v.starts_with("26.1.") || v.starts_with("26.2."))
        .cloned()
        .collect();
    kept.extend(calendar.iter().rev().take(6).rev().cloned());
    write(
        "neoforge_maven_metadata.xml",
        &maven_document("net.neoforged", "neoforge", &kept),
    )
}

fn maven_versions(xml: &str) -> Vec<String> {
    let mut versions = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<version>") {
        let after = &rest[start + "<version>".len()..];
        let Some(end) = after.find("</version>") else {
            break;
        };
        versions.push(after[..end].trim().to_string());
        rest = &after[end..];
    }
    versions
}

fn maven_document(group: &str, artifact: &str, versions: &[String]) -> String {
    let body = versions
        .iter()
        .map(|v| format!("      <version>{v}</version>"))
        .collect::<Vec<_>>()
        .join("\n");
    let latest = versions.last().cloned().unwrap_or_default();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <metadata>\n\
         \x20 <groupId>{group}</groupId>\n\
         \x20 <artifactId>{artifact}</artifactId>\n\
         \x20 <versioning>\n\
         \x20   <latest>{latest}</latest>\n\
         \x20   <release>{latest}</release>\n\
         \x20   <versions>\n{body}\n\
         \x20   </versions>\n\
         \x20 </versioning>\n\
         </metadata>\n"
    )
}
