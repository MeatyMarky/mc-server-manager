//! What each `server.properties` key means, so the editor can show a typed
//! control instead of a text box.
//!
//! The list covers vanilla's keys. Anything not listed — Paper's extras, a
//! fork's own settings, a typo — is still editable as raw text, because this
//! app never decides a key is invalid just because it does not recognise it.

use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub enum ValueKind {
    Bool,
    Int {
        #[ts(type = "number | null")]
        min: Option<i64>,
        #[ts(type = "number | null")]
        max: Option<i64>,
    },
    Enum {
        options: Vec<String>,
    },
    Text,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct KeyInfo {
    pub key: String,
    pub kind: ValueKind,
    pub description: String,
    pub default: Option<String>,
    /// A key this build does not know: shown with a raw text editor.
    pub known: bool,
    /// Changing it needs a restart, which is true of every server property.
    pub group: String,
}

fn boolean(key: &str, group: &str, default: &str, description: &str) -> KeyInfo {
    KeyInfo {
        key: key.to_string(),
        kind: ValueKind::Bool,
        description: description.to_string(),
        default: Some(default.to_string()),
        known: true,
        group: group.to_string(),
    }
}

fn int(key: &str, group: &str, default: &str, min: Option<i64>, max: Option<i64>, description: &str) -> KeyInfo {
    KeyInfo {
        key: key.to_string(),
        kind: ValueKind::Int { min, max },
        description: description.to_string(),
        default: Some(default.to_string()),
        known: true,
        group: group.to_string(),
    }
}

fn choice(key: &str, group: &str, default: &str, options: &[&str], description: &str) -> KeyInfo {
    KeyInfo {
        key: key.to_string(),
        kind: ValueKind::Enum {
            options: options.iter().map(|option| option.to_string()).collect(),
        },
        description: description.to_string(),
        default: Some(default.to_string()),
        known: true,
        group: group.to_string(),
    }
}

fn text(key: &str, group: &str, default: &str, description: &str) -> KeyInfo {
    KeyInfo {
        key: key.to_string(),
        kind: ValueKind::Text,
        description: description.to_string(),
        default: Some(default.to_string()),
        known: true,
        group: group.to_string(),
    }
}

/// Everything vanilla writes into a fresh `server.properties`.
pub fn known_keys() -> Vec<KeyInfo> {
    vec![
        // Network
        int("server-port", "Network", "25565", Some(1), Some(65535), "TCP port the server listens on."),
        text("server-ip", "Network", "", "Interface to bind to. Empty means all interfaces."),
        boolean("online-mode", "Network", "true", "Verify players against Mojang's session servers. Turning this off lets anyone use any username."),
        boolean("prevent-proxy-connections", "Network", "false", "Reject players connecting through a proxy."),
        int("network-compression-threshold", "Network", "256", Some(-1), None, "Packets larger than this are compressed. -1 disables compression."),
        int("rate-limit", "Network", "0", Some(0), None, "Packets per second before a player is kicked. 0 disables the limit."),
        int("max-players", "Network", "20", Some(0), None, "How many players can be online at once."),
        int("player-idle-timeout", "Network", "0", Some(0), None, "Minutes before an idle player is kicked. 0 never kicks."),
        // Gameplay
        choice("gamemode", "Gameplay", "survival", &["survival", "creative", "adventure", "spectator"], "Game mode new players start in."),
        boolean("force-gamemode", "Gameplay", "false", "Put players back into the default game mode when they join."),
        choice("difficulty", "Gameplay", "easy", &["peaceful", "easy", "normal", "hard"], "How hostile the world is."),
        boolean("hardcore", "Gameplay", "false", "Death is permanent and players are banned from the world."),
        boolean("pvp", "Gameplay", "true", "Allow players to damage each other."),
        boolean("allow-flight", "Gameplay", "false", "Permit flight in survival, which anti-cheat would otherwise kick for."),
        boolean("allow-nether", "Gameplay", "true", "Allow travel to the Nether."),
        boolean("spawn-monsters", "Gameplay", "true", "Spawn hostile mobs."),
        boolean("spawn-npcs", "Gameplay", "true", "Spawn villagers."),
        boolean("spawn-animals", "Gameplay", "true", "Spawn passive mobs."),
        int("spawn-protection", "Gameplay", "16", Some(0), None, "Radius around spawn that only operators can build in."),
        // World
        text("level-name", "World", "world", "Folder name of the world to load."),
        text("level-seed", "World", "", "Seed for generating a new world. Ignored once the world exists."),
        choice("level-type", "World", "minecraft:normal", &["minecraft:normal", "minecraft:flat", "minecraft:large_biomes", "minecraft:amplified", "minecraft:single_biome_surface"], "World generator to use for a new world."),
        text("generator-settings", "World", "{}", "JSON settings for the generator, used by flat and single-biome worlds."),
        boolean("generate-structures", "World", "true", "Generate villages, temples and other structures."),
        int("max-world-size", "World", "29999984", Some(1), Some(29999984), "World border radius in blocks."),
        int("view-distance", "World", "10", Some(2), Some(32), "Chunks sent to each player. The biggest single lever on server load."),
        int("simulation-distance", "World", "10", Some(3), Some(32), "Chunks that keep ticking around each player."),
        int("entity-broadcast-range-percentage", "World", "100", Some(10), Some(1000), "How far entities are visible, as a percentage of the default."),
        int("max-chained-neighbor-updates", "World", "1000000", None, None, "Cap on chained block updates, which limits redstone lag machines."),
        // Access
        boolean("white-list", "Access", "false", "Only players on the whitelist may join."),
        boolean("enforce-whitelist", "Access", "false", "Kick players who are online but not whitelisted."),
        boolean("enforce-secure-profile", "Access", "true", "Require signed chat profiles."),
        int("op-permission-level", "Access", "4", Some(1), Some(4), "Permission level granted by /op."),
        int("function-permission-level", "Access", "2", Some(1), Some(4), "Permission level functions run at."),
        boolean("broadcast-console-to-ops", "Access", "true", "Show console command output to operators."),
        boolean("broadcast-rcon-to-ops", "Access", "true", "Show RCON command output to operators."),
        // Presentation
        text("motd", "Presentation", "A Minecraft Server", "Message shown in the server list. Supports colour codes and non-ASCII text."),
        boolean("hide-online-players", "Presentation", "false", "Hide the player list from the server list ping."),
        boolean("enable-status", "Presentation", "true", "Answer server list pings at all."),
        text("resource-pack", "Presentation", "", "URL of a resource pack to offer players."),
        text("resource-pack-sha1", "Presentation", "", "SHA-1 of the resource pack, so clients can cache it."),
        text("resource-pack-prompt", "Presentation", "", "Message shown when offering the resource pack."),
        boolean("require-resource-pack", "Presentation", "false", "Disconnect players who decline the resource pack."),
        // Operations
        boolean("enable-command-block", "Operations", "false", "Allow command blocks to run."),
        boolean("enable-jmx-monitoring", "Operations", "false", "Expose JMX metrics."),
        boolean("enable-rcon", "Operations", "false", "Enable the RCON remote console."),
        int("rcon.port", "Operations", "25575", Some(1), Some(65535), "Port RCON listens on."),
        text("rcon.password", "Operations", "", "RCON password. Leave empty to keep RCON off."),
        boolean("enable-query", "Operations", "false", "Enable the GameSpy4 query protocol."),
        int("query.port", "Operations", "25565", Some(1), Some(65535), "Port the query listener uses."),
        boolean("sync-chunk-writes", "Operations", "true", "Write chunks synchronously. Safer, slower."),
        boolean("use-native-transport", "Operations", "true", "Use Linux-specific networking optimisations."),
        int("max-tick-time", "Operations", "60000", Some(-1), None, "Milliseconds a tick may take before the watchdog kills the server. -1 disables it."),
        boolean("log-ips", "Operations", "true", "Write player IP addresses to the log."),
        text("initial-enabled-packs", "Operations", "vanilla", "Data packs enabled when the world is created."),
        text("initial-disabled-packs", "Operations", "", "Data packs disabled when the world is created."),
        text("text-filtering-config", "Operations", "", "Path to a text filtering configuration."),
        int("text-filtering-version", "Operations", "0", None, None, "Version of the text filtering configuration."),
        boolean("accepts-transfers", "Operations", "false", "Accept players transferred from another server."),
        int("pause-when-empty-seconds", "Operations", "60", Some(0), None, "Seconds with no players before the server pauses ticking."),
    ]
}

/// Metadata for a key, falling back to a raw text editor for anything unknown.
pub fn describe(key: &str) -> KeyInfo {
    known_keys()
        .into_iter()
        .find(|info| info.key == key)
        .unwrap_or_else(|| KeyInfo {
            key: key.to_string(),
            kind: ValueKind::Text,
            description: "Not a key this build knows about — edited as raw text.".to_string(),
            default: None,
            known: false,
            group: "Other".to_string(),
        })
}

/// Checks a value against the key's type. Returns the reason it is unusable.
pub fn validate(info: &KeyInfo, value: &str) -> Option<String> {
    match &info.kind {
        ValueKind::Bool => match value {
            "true" | "false" => None,
            other => Some(format!("{} must be true or false, not \"{other}\"", info.key)),
        },
        ValueKind::Int { min, max } => {
            let Ok(parsed) = value.trim().parse::<i64>() else {
                return Some(format!("{} must be a whole number", info.key));
            };
            if let Some(min) = min {
                if parsed < *min {
                    return Some(format!("{} cannot be below {min}", info.key));
                }
            }
            if let Some(max) = max {
                if parsed > *max {
                    return Some(format!("{} cannot be above {max}", info.key));
                }
            }
            None
        }
        // Enums stay advisory: servers accept values this build has not heard of
        // (a new level type, a modded game mode), so an unknown option is not
        // rejected — the dropdown simply gains it.
        ValueKind::Enum { .. } | ValueKind::Text => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_keys_are_unique_and_described() {
        let keys = known_keys();
        let mut names: Vec<&str> = keys.iter().map(|info| info.key.as_str()).collect();
        names.sort();
        let unique: std::collections::HashSet<&&str> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "a key is listed twice");

        for info in &keys {
            assert!(!info.description.is_empty(), "{} has no description", info.key);
            assert!(!info.group.is_empty());
            assert!(info.known);
        }
    }

    #[test]
    fn the_keys_that_matter_most_are_typed() {
        assert!(matches!(describe("pvp").kind, ValueKind::Bool));
        assert!(matches!(describe("max-players").kind, ValueKind::Int { .. }));
        assert!(matches!(describe("difficulty").kind, ValueKind::Enum { .. }));
        assert!(matches!(describe("motd").kind, ValueKind::Text));
    }

    #[test]
    fn unknown_keys_fall_back_to_raw_text() {
        let info = describe("paper.custom-thing");
        assert!(!info.known);
        assert!(matches!(info.kind, ValueKind::Text));
        assert_eq!(info.group, "Other");
        assert!(info.default.is_none());
    }

    #[test]
    fn booleans_reject_anything_else() {
        let info = describe("pvp");
        assert_eq!(validate(&info, "true"), None);
        assert_eq!(validate(&info, "false"), None);
        assert!(validate(&info, "yes").unwrap().contains("true or false"));
    }

    #[test]
    fn integers_respect_their_range() {
        let info = describe("view-distance");
        assert_eq!(validate(&info, "10"), None);
        assert!(validate(&info, "1").unwrap().contains("below 2"));
        assert!(validate(&info, "64").unwrap().contains("above 32"));
        assert!(validate(&info, "ten").unwrap().contains("whole number"));
    }

    #[test]
    fn an_unbounded_integer_only_checks_the_type() {
        let info = describe("max-chained-neighbor-updates");
        assert_eq!(validate(&info, "-5"), None);
        assert_eq!(validate(&info, "99999999"), None);
        assert!(validate(&info, "").is_some());
    }

    #[test]
    fn enums_and_text_accept_values_this_build_has_not_heard_of() {
        // A modded level type must not be rejected by our own list.
        assert_eq!(validate(&describe("level-type"), "terralith:overworld"), None);
        assert_eq!(validate(&describe("motd"), "§aColoured čšž"), None);
        assert_eq!(validate(&describe("paper.custom"), "anything"), None);
    }
}
