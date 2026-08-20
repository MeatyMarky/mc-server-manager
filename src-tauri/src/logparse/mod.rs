//! Turning server output into structured lines and events.
//!
//! The three families print differently:
//!
//! ```text
//! vanilla  [12:34:56] [Server thread/INFO]: Done (7.214s)! For help, type "help"
//! paper    [12:34:56 INFO]: Done (7.214s)! For help, type "help"
//! forge    [12:34:56] [main/INFO] [net.minecraftforge.fml.loading:52]: Loading
//! fabric   [12:34:56] [main/INFO] (FabricLoader) Loading 42 mods
//! log4j    2026-08-18 12:34:56,123 main INFO  Message
//! ```
//!
//! Anything that fails to match is still emitted verbatim — losing a line is
//! worse than losing its structure.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
    /// Written to stderr with no recognizable level, or an unparsed line.
    Raw,
}

impl LogLevel {
    pub fn parse(token: &str) -> Option<Self> {
        match token.trim().to_ascii_uppercase().as_str() {
            "TRACE" => Some(LogLevel::Trace),
            "DEBUG" => Some(LogLevel::Debug),
            "INFO" => Some(LogLevel::Info),
            "WARN" | "WARNING" => Some(LogLevel::Warn),
            "ERROR" | "SEVERE" => Some(LogLevel::Error),
            "FATAL" => Some(LogLevel::Fatal),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Trace => "trace",
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
            LogLevel::Fatal => "fatal",
            LogLevel::Raw => "raw",
        }
    }
}

/// One console line, structured as far as the format allows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/lib/bindings/")]
pub struct ParsedLine {
    /// Monotonic per-instance sequence number, assigned by the ring buffer.
    #[ts(type = "number")]
    pub seq: u64,
    /// Wall-clock time this line was captured (RFC3339 UTC), not the server's stamp.
    pub captured_at: String,
    /// The server's own timestamp text (`12:34:56`), when it printed one.
    pub timestamp: Option<String>,
    pub level: LogLevel,
    pub thread: Option<String>,
    /// The message with prefixes stripped, or the whole line when unparsed.
    pub message: String,
    /// The original line, always kept verbatim for copy and search.
    pub raw: String,
    /// True when the line came from stderr.
    pub stderr: bool,
}

/// Things the supervisor reacts to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogEvent {
    /// "Done (7.214s)! For help, type help" — the server is accepting players.
    Ready { took: Option<String> },
    Stopping,
    PlayerJoined { name: String, uuid: Option<String> },
    PlayerLeft { name: String },
    /// The server printed a UUID assignment line, which is how a name maps to a UUID.
    PlayerUuid { name: String, uuid: String },
    /// Port already taken; the message is kept for the UI.
    PortInUse { detail: String },
    /// A crash report or an unrecoverable exception.
    Crash { detail: String },
    /// World save finished; backups wait for this.
    Saved,
    /// The server's class files are newer than the JVM running them.
    ///
    /// The JVM reports this in class file versions, which nobody thinks in:
    /// "class file version 69.0 … up to 61.0" means the server needs Java 25
    /// and got Java 17. Both numbers are translated here so the UI never has to.
    ClassVersion {
        /// Java feature version the server was built for.
        needs: i64,
        /// Java feature version that tried to run it.
        found: i64,
        /// The class named in the message, for the technical detail.
        class_name: Option<String>,
    },
}

/// Class file 45 is Java 1.1, and every release since has added one.
pub fn java_from_class_version(class_version: f64) -> i64 {
    (class_version.trunc() as i64) - 44
}

/// Reads both numbers out of an `UnsupportedClassVersionError`.
///
/// The message has been stable for two decades: "X has been compiled by a more
/// recent version of the Java Runtime (class file version A), this version of
/// the Java Runtime only recognizes class file versions up to B".
pub fn parse_class_version_error(message: &str) -> Option<LogEvent> {
    if !message.contains("UnsupportedClassVersionError")
        && !message.contains("class file version")
    {
        return None;
    }

    let numbers: Vec<f64> = message
        .split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .filter(|token| token.contains('.'))
        .filter_map(|token| token.parse::<f64>().ok())
        .filter(|value| (45.0..=200.0).contains(value))
        .collect();
    let (needs, found) = match numbers.as_slice() {
        [needs, found, ..] => (*needs, *found),
        _ => return None,
    };

    let class_name = message
        .split_whitespace()
        .find(|token| token.contains('/') && !token.contains("://"))
        .map(|token| token.trim_end_matches(':').replace('/', "."));

    Some(LogEvent::ClassVersion {
        needs: java_from_class_version(needs),
        found: java_from_class_version(found),
        class_name,
    })
}

/// Strips a `[…]` or `(…)` prefix, returning its contents and the rest.
fn take_bracketed(input: &str, open: char, close: char) -> Option<(&str, &str)> {
    let rest = input.strip_prefix(open)?;
    let end = rest.find(close)?;
    Some((&rest[..end], rest[end + close.len_utf8()..].trim_start()))
}

fn is_timestamp(token: &str) -> bool {
    // 12:34:56 or 12:34:56.123
    let mut parts = token.split(':');
    let ok = |p: Option<&str>| {
        p.map(|value| {
            let value = value.split('.').next().unwrap_or(value);
            !value.is_empty() && value.chars().all(|c| c.is_ascii_digit())
        })
        .unwrap_or(false)
    };
    ok(parts.next()) && ok(parts.next()) && ok(parts.next()) && parts.next().is_none()
}

/// Parses one line of server output. `stderr` marks which stream it came from.
///
/// A line that matches nothing keeps its full text as the message with level
/// `Raw`; stack-trace continuations ("\tat net.minecraft…") are exactly that.
pub fn parse_line(raw: &str, stderr: bool) -> (Option<String>, LogLevel, Option<String>, String) {
    let line = raw.trim_end_matches(['\r', '\n']);
    let trimmed = line.trim_start();

    // log4j plain layout: 2026-08-18 12:34:56,123 main INFO  Message
    if let Some(parsed) = parse_log4j(trimmed) {
        return parsed;
    }

    let Some((first, rest)) = take_bracketed(trimmed, '[', ']') else {
        return (
            None,
            unformatted_level(trimmed, stderr),
            None,
            line.to_string(),
        );
    };

    // Paper: [12:34:56 INFO]: message
    if let Some((stamp, level)) = first.split_once(' ') {
        if is_timestamp(stamp) {
            if let Some(level) = LogLevel::parse(level) {
                return (
                    Some(stamp.to_string()),
                    level,
                    None,
                    rest.trim_start_matches(':').trim().to_string(),
                );
            }
        }
    }

    if !is_timestamp(first) {
        return (
            None,
            unformatted_level(trimmed, stderr),
            None,
            line.to_string(),
        );
    }
    let timestamp = Some(first.to_string());

    // Vanilla/Forge/Fabric: [12:34:56] [thread/LEVEL] [logger]: message
    let Some((second, mut rest)) = take_bracketed(rest, '[', ']') else {
        let message = rest.trim_start_matches(':').trim().to_string();
        return (
            timestamp,
            unformatted_level(&message, stderr),
            None,
            message,
        );
    };

    let (thread, level) = match second.rsplit_once('/') {
        Some((thread, level)) => (
            Some(thread.to_string()),
            LogLevel::parse(level).unwrap_or_else(|| default_level(stderr)),
        ),
        None => (Some(second.to_string()), default_level(stderr)),
    };

    // Forge adds a third bracket with the logger name, Fabric a parenthesised
    // one ("(FabricLoader)", "(Minecraft)"). Both are logger names, not message.
    if rest.starts_with('[') {
        if let Some((_logger, after)) = take_bracketed(rest, '[', ']') {
            rest = after;
        }
    } else if rest.starts_with('(') {
        if let Some((logger, after)) = take_bracketed(rest, '(', ')') {
            // Only when it really looks like a logger name: no spaces inside.
            if !logger.contains(' ') && !logger.is_empty() {
                rest = after;
            }
        }
    }

    (
        timestamp,
        level,
        thread,
        rest.trim_start_matches(':').trim().to_string(),
    )
}

/// The level of a line that carries no recognised log format.
///
/// A line's own words come first: the JVM writes `WARNING: ...` and
/// `ERROR: ...` to stderr, and every one of those was being shown as an error
/// purely because of the stream it arrived on — the `sun.misc.Unsafe`
/// deprecation notices Minecraft 26 prints on every start are warnings that
/// say so. The stream is only a fallback for a line that declares nothing.
fn default_level(stderr: bool) -> LogLevel {
    if stderr {
        LogLevel::Error
    } else {
        LogLevel::Raw
    }
}

/// The level a bare line declares about itself, if any.
///
/// Matches the shape the JVM and `java.util.logging` use — a level word, then a
/// colon, at the very start of the line. Anything further in is prose: "the
/// error was handled" is not an error line.
pub fn declared_level(line: &str) -> Option<LogLevel> {
    let trimmed = line.trim_start();
    let (word, rest) = trimmed.split_once(':')?;
    if word.is_empty() || word.len() > 8 || word.contains(char::is_whitespace) {
        return None;
    }
    // "WARNING:" with nothing after it is still a warning; "WARNING:foo" is not
    // a log line shape this app should reinterpret.
    if !(rest.is_empty() || rest.starts_with(' ')) {
        return None;
    }
    LogLevel::parse(word)
}

/// The level for a line no format matched: what it says about itself, then the
/// stream it came from.
fn unformatted_level(line: &str, stderr: bool) -> LogLevel {
    declared_level(line).unwrap_or_else(|| default_level(stderr))
}

fn parse_log4j(line: &str) -> Option<(Option<String>, LogLevel, Option<String>, String)> {
    // 2026-08-18 12:34:56,123 main INFO  Loading
    let mut parts = line.splitn(5, ' ');
    let date = parts.next()?;
    if date.len() != 10 || !date.starts_with("20") {
        return None;
    }
    let time = parts.next()?;
    if !is_timestamp(time.split(',').next().unwrap_or(time)) {
        return None;
    }
    let thread = parts.next()?;
    let level = LogLevel::parse(parts.next()?)?;
    let message = parts.next().unwrap_or("").trim_start().to_string();
    Some((
        Some(time.to_string()),
        level,
        Some(thread.to_string()),
        message,
    ))
}

/// What the console shows instead of the first boot's missing-properties error.
pub const MISSING_PROPERTIES_NOTE: &str =
    "No server.properties yet — the server is about to write one.";

/// The header of the "there is no properties file" complaint.
///
/// A server with no `server.properties` logs this at ERROR with a full
/// `NoSuchFileException` trace and then starts perfectly normally, because the
/// file does not exist until it writes one. On a first boot that is the
/// expected sequence of events; on any later boot the file has gone missing,
/// which is worth every bit of the noise.
pub fn is_missing_properties_header(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("failed to load properties from file") && lower.contains("server.properties")
}

/// Whether a line is the start of a fresh log line from the server.
///
/// Either a bracketed timestamp (vanilla, Paper, Forge, Fabric) or the log4j
/// plain layout. Used to decide where a stack trace ends, because everything
/// in between belongs to the exception.
pub fn starts_a_log_line(line: &str) -> bool {
    let trimmed = line.trim_end_matches(['\r', '\n']);
    trimmed.starts_with('[') || parse_log4j(trimmed).is_some()
}

/// Whether a line is the continuation of the exception above it.
///
/// Java prints the exception class first, then frames indented with a tab or
/// spaces, then optional `Caused by:` and `... 12 more` lines. None of them
/// carry a log prefix, which is what separates them from the next real line.
pub fn is_exception_continuation(line: &str) -> bool {
    let trimmed = line.trim_end_matches(['\r', '\n']);
    let body = trimmed.trim_start();

    if body.is_empty() {
        return false;
    }
    // A new log line starts with its own bracketed stamp or a log4j date.
    if trimmed.starts_with('[') || parse_log4j(trimmed).is_some() {
        return false;
    }
    // Indented frames: "\tat java.base/…" or "    at java.base/…".
    if trimmed.starts_with('\t') || trimmed.starts_with("  ") {
        return true;
    }
    body.starts_with("at ")
        || body.starts_with("Caused by:")
        || body.starts_with("Suppressed:")
        || body.starts_with("...")
        // The exception line itself: "java.nio.file.NoSuchFileException: …".
        || (body.starts_with("java.") && body.contains("Exception"))
}

/// Recognizes the events the supervisor and the players view care about.
///
/// Matching is done on the *message* (prefixes already stripped) so the same
/// rules work across vanilla, Paper, Forge and Fabric.
pub fn detect_event(message: &str) -> Option<LogEvent> {
    let lower = message.to_ascii_lowercase();

    if lower.starts_with("done (") && lower.contains("for help") {
        let took = message
            .split_once('(')
            .and_then(|(_, rest)| rest.split_once(')'))
            .map(|(inside, _)| inside.to_string());
        return Some(LogEvent::Ready { took });
    }

    if lower.starts_with("stopping the server") || lower.starts_with("stopping server") {
        return Some(LogEvent::Stopping);
    }

    if lower.contains("saved the game") || lower.starts_with("saved the world") {
        return Some(LogEvent::Saved);
    }

    // "Failed to bind to port" (vanilla) / "**** FAILED TO BIND TO PORT!" (paper)
    if lower.contains("failed to bind to port") || lower.contains("address already in use") {
        return Some(LogEvent::PortInUse {
            detail: message.to_string(),
        });
    }

    if let Some(event) = parse_class_version_error(message) {
        return Some(event);
    }

    if lower.contains("crash report")
        || lower.starts_with("exception in server tick loop")
        || lower.starts_with("exception in thread")
        || lower.contains("encountered an unexpected exception")
    {
        return Some(LogEvent::Crash {
            detail: message.to_string(),
        });
    }

    // "Notch[/127.0.0.1:52222] logged in with entity id 42 at (…)"
    if let Some(name) = message.split('[').next() {
        if lower.contains("logged in with entity id") && !name.is_empty() {
            return Some(LogEvent::PlayerJoined {
                name: name.trim().to_string(),
                uuid: None,
            });
        }
    }

    // "UUID of player Notch is 069a79f4-44e9-4726-a5be-fca90e38aaf5"
    if let Some(rest) = strip_prefix_ignore_case(message, "UUID of player ") {
        if let Some((name, uuid)) = rest.split_once(" is ") {
            return Some(LogEvent::PlayerUuid {
                name: name.trim().to_string(),
                uuid: uuid.trim().to_string(),
            });
        }
    }

    // "Notch joined the game" / "Notch left the game"
    if let Some(name) = lower.strip_suffix(" joined the game") {
        return Some(LogEvent::PlayerJoined {
            name: message[..name.len()].trim().to_string(),
            uuid: None,
        });
    }
    if let Some(name) = lower.strip_suffix(" left the game") {
        return Some(LogEvent::PlayerLeft {
            name: message[..name.len()].trim().to_string(),
        });
    }

    // "Notch lost connection: Disconnected"
    if let Some((name, _)) = message.split_once(" lost connection:") {
        if !name.is_empty() && !name.contains(' ') {
            return Some(LogEvent::PlayerLeft {
                name: name.trim().to_string(),
            });
        }
    }

    None
}

fn strip_prefix_ignore_case<'a>(input: &'a str, prefix: &str) -> Option<&'a str> {
    if input.len() >= prefix.len() && input[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&input[prefix.len()..])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &str) -> (Option<String>, LogLevel, Option<String>, String) {
        parse_line(raw, false)
    }

    #[test]
    fn parses_vanilla_lines() {
        let (ts, level, thread, message) =
            parse(r#"[12:34:56] [Server thread/INFO]: Done (7.214s)! For help, type "help""#);
        assert_eq!(ts.as_deref(), Some("12:34:56"));
        assert_eq!(level, LogLevel::Info);
        assert_eq!(thread.as_deref(), Some("Server thread"));
        assert_eq!(message, r#"Done (7.214s)! For help, type "help""#);
    }

    #[test]
    fn parses_paper_lines() {
        let (ts, level, thread, message) =
            parse("[12:34:56 WARN]: Legacy plugin detected, this is unsupported");
        assert_eq!(ts.as_deref(), Some("12:34:56"));
        assert_eq!(level, LogLevel::Warn);
        assert_eq!(thread, None, "Paper's default layout omits the thread");
        assert_eq!(message, "Legacy plugin detected, this is unsupported");
    }

    #[test]
    fn parses_forge_lines_with_a_logger_bracket() {
        let (ts, level, thread, message) = parse(
            "[12:34:56] [main/INFO] [net.minecraftforge.fml.loading.FMLLoader/CORE]: Loading Forge",
        );
        assert_eq!(ts.as_deref(), Some("12:34:56"));
        assert_eq!(level, LogLevel::Info);
        assert_eq!(thread.as_deref(), Some("main"));
        assert_eq!(message, "Loading Forge", "the logger bracket is dropped");
    }

    #[test]
    fn parses_fabric_lines() {
        let (_, level, thread, message) =
            parse("[12:34:56] [main/INFO] (FabricLoader) Loading 42 mods");
        assert_eq!(level, LogLevel::Info);
        assert_eq!(thread.as_deref(), Some("main"));
        assert_eq!(message, "Loading 42 mods", "the logger marker is dropped");

        // Fabric prefixes even the readiness line, which detection depends on.
        let (_, _, _, message) =
            parse(r#"[13:00:14] [Server thread/INFO] (Minecraft) Done (8.331s)! For help, type "help""#);
        assert_eq!(message, r#"Done (8.331s)! For help, type "help""#);

        // A parenthesised phrase that is not a logger name stays in the message.
        let (_, _, _, message) = parse("[12:34:56] [main/INFO] (not a logger) text");
        assert_eq!(message, "(not a logger) text");
    }

    #[test]
    fn parses_the_log4j_plain_layout() {
        let (ts, level, thread, message) =
            parse("2026-08-18 12:34:56,123 main WARN Advanced terminal features are not available");
        assert_eq!(ts.as_deref(), Some("12:34:56,123"));
        assert_eq!(level, LogLevel::Warn);
        assert_eq!(thread.as_deref(), Some("main"));
        assert_eq!(message, "Advanced terminal features are not available");
    }

    #[test]
    fn keeps_unparsable_lines_verbatim() {
        let (ts, level, thread, message) = parse("\tat net.minecraft.server.Main.main(Main.java:1)");
        assert_eq!(ts, None);
        assert_eq!(level, LogLevel::Raw);
        assert_eq!(thread, None);
        assert_eq!(message, "\tat net.minecraft.server.Main.main(Main.java:1)");
    }

    #[test]
    fn stderr_lines_default_to_error() {
        let (_, level, _, message) = parse_line("Error: A JNI error has occurred", true);
        assert_eq!(level, LogLevel::Error);
        assert_eq!(message, "Error: A JNI error has occurred");
    }

    #[test]
    fn handles_both_line_endings() {
        let with_crlf = parse("[12:34:56] [Server thread/INFO]: Preparing spawn area\r\n");
        let with_lf = parse("[12:34:56] [Server thread/INFO]: Preparing spawn area\n");
        assert_eq!(with_crlf, with_lf);
        assert_eq!(with_crlf.3, "Preparing spawn area");
    }

    #[test]
    fn levels_cover_the_spellings_servers_use() {
        assert_eq!(LogLevel::parse("warning"), Some(LogLevel::Warn));
        assert_eq!(LogLevel::parse("SEVERE"), Some(LogLevel::Error));
        assert_eq!(LogLevel::parse("info"), Some(LogLevel::Info));
        assert_eq!(LogLevel::parse("nonsense"), None);
    }

    #[test]
    fn detects_readiness_with_the_startup_time() {
        let event = detect_event(r#"Done (7.214s)! For help, type "help""#);
        assert_eq!(
            event,
            Some(LogEvent::Ready {
                took: Some("7.214s".to_string())
            })
        );
    }

    #[test]
    fn detects_shutdown_and_saves() {
        assert_eq!(detect_event("Stopping the server"), Some(LogEvent::Stopping));
        assert_eq!(detect_event("Stopping server"), Some(LogEvent::Stopping));
        assert_eq!(detect_event("Saved the game"), Some(LogEvent::Saved));
    }

    #[test]
    fn detects_players_across_formats() {
        assert_eq!(
            detect_event("UUID of player Notch is 069a79f4-44e9-4726-a5be-fca90e38aaf5"),
            Some(LogEvent::PlayerUuid {
                name: "Notch".into(),
                uuid: "069a79f4-44e9-4726-a5be-fca90e38aaf5".into()
            })
        );
        assert_eq!(
            detect_event("Notch[/127.0.0.1:52222] logged in with entity id 42 at (0.5, 64.0, 0.5)"),
            Some(LogEvent::PlayerJoined {
                name: "Notch".into(),
                uuid: None
            })
        );
        assert_eq!(
            detect_event("Notch joined the game"),
            Some(LogEvent::PlayerJoined {
                name: "Notch".into(),
                uuid: None
            })
        );
        assert_eq!(
            detect_event("Notch left the game"),
            Some(LogEvent::PlayerLeft {
                name: "Notch".into()
            })
        );
        assert_eq!(
            detect_event("Notch lost connection: Disconnected"),
            Some(LogEvent::PlayerLeft {
                name: "Notch".into()
            })
        );
    }

    #[test]
    fn detects_a_taken_port() {
        let event = detect_event("FAILED TO BIND TO PORT! Perhaps a server is already running on that port?");
        assert!(matches!(event, Some(LogEvent::PortInUse { .. })));
        assert!(matches!(
            detect_event("java.net.BindException: Address already in use: bind"),
            Some(LogEvent::PortInUse { .. })
        ));
    }

    #[test]
    fn detects_crashes() {
        assert!(matches!(
            detect_event("Exception in server tick loop"),
            Some(LogEvent::Crash { .. })
        ));
        assert!(matches!(
            detect_event("This crash report has been saved to: ./crash-reports/crash.txt"),
            Some(LogEvent::Crash { .. })
        ));
        assert!(matches!(
            detect_event("The server encountered an unexpected exception"),
            Some(LogEvent::Crash { .. })
        ));
    }

    #[test]
    fn ordinary_chatter_produces_no_event() {
        assert_eq!(detect_event("Preparing spawn area: 42%"), None);
        assert_eq!(detect_event("<Notch> hello world"), None);
        assert_eq!(detect_event(""), None);
    }

    /// The recorded samples exercise each family end to end: every line must
    /// parse without panicking, and the known events must be found.
    #[test]
    fn recorded_samples_parse_and_yield_their_events() {
        for (file, expect_ready) in [
            ("log_vanilla.txt", true),
            ("log_paper.txt", true),
            ("log_forge.txt", true),
            ("log_fabric.txt", true),
            ("log_crash.txt", false),
        ] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(file);
            let contents = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("missing fixture {}: {e}", path.display()));

            let mut ready = false;
            let mut events = 0;
            for raw in contents.lines() {
                let (_, _, _, message) = parse_line(raw, false);
                assert!(!message.is_empty() || raw.trim().is_empty(), "dropped: {raw}");
                if let Some(event) = detect_event(&message) {
                    events += 1;
                    if matches!(event, LogEvent::Ready { .. }) {
                        ready = true;
                    }
                }
            }
            assert_eq!(ready, expect_ready, "{file} readiness detection");
            assert!(events > 0, "{file} produced no events at all");
        }
    }

    #[test]
    fn crash_sample_reports_a_crash_and_the_port_sample_a_port_clash() {
        let read = |name: &str| {
            std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures")
                    .join(name),
            )
            .unwrap()
        };

        let crash = read("log_crash.txt");
        assert!(crash.lines().any(|raw| {
            let (_, _, _, message) = parse_line(raw, false);
            matches!(detect_event(&message), Some(LogEvent::Crash { .. }))
        }));

        let port = read("log_port_in_use.txt");
        assert!(port.lines().any(|raw| {
            let (_, _, _, message) = parse_line(raw, false);
            matches!(detect_event(&message), Some(LogEvent::PortInUse { .. }))
        }));
    }

    /// The real line this app was shown, from a Fabric 26.2 server on Java 17.
    const CLASS_VERSION_ERROR: &str = "Caused by: java.lang.UnsupportedClassVersionError: \
net/minecraft/bundler/Main has been compiled by a more recent version of the Java Runtime \
(class file version 69.0), this version of the Java Runtime only recognizes class file \
versions up to 61.0";

    #[test]
    fn class_file_versions_become_java_versions() {
        // 45 is Java 1.1, and every release since adds one.
        assert_eq!(java_from_class_version(45.0), 1);
        assert_eq!(java_from_class_version(52.0), 8);
        assert_eq!(java_from_class_version(61.0), 17);
        assert_eq!(java_from_class_version(65.0), 21);
        assert_eq!(java_from_class_version(69.0), 25);
    }

    #[test]
    fn an_unsupported_class_version_is_translated_into_java_versions() {
        let event = detect_event(CLASS_VERSION_ERROR).expect("recognised");
        match event {
            LogEvent::ClassVersion {
                needs,
                found,
                class_name,
            } => {
                assert_eq!(needs, 25, "class file 69 is Java 25");
                assert_eq!(found, 17, "class file 61 is Java 17");
                assert_eq!(class_name.as_deref(), Some("net.minecraft.bundler.Main"));
            }
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[test]
    fn the_same_message_is_recognised_without_the_exception_name() {
        // Some launchers print only the tail of the message.
        let short = "Main has been compiled by a more recent version of the Java Runtime \
                     (class file version 65.0), this version of the Java Runtime only \
                     recognizes class file versions up to 61.0";
        assert!(matches!(
            detect_event(short),
            Some(LogEvent::ClassVersion { needs: 21, found: 17, .. })
        ));
    }

    #[test]
    fn ordinary_lines_are_not_mistaken_for_a_class_version_error() {
        assert!(!matches!(
            detect_event("Done (7.214s)! For help, type \"help\""),
            Some(LogEvent::ClassVersion { .. })
        ));
        assert!(!matches!(
            detect_event("Loading 42 mods, version 1.21.4"),
            Some(LogEvent::ClassVersion { .. })
        ));
        // A mention with no numbers in it cannot be translated, so it is not
        // claimed as one.
        assert!(parse_class_version_error("UnsupportedClassVersionError somewhere").is_none());
    }

    #[test]
    fn a_jvm_warning_on_stderr_is_a_warning_not_an_error() {
        // Minecraft 26 prints these on every start, and every one of them was
        // being shown in red because it arrived on stderr.
        let fixture = include_str!("../../tests/fixtures/log_jvm_stderr.txt");
        let unsafe_line = fixture
            .lines()
            .find(|line| line.contains("sun.misc.Unsafe::objectFieldOffset has been called"))
            .expect("the fixture has the deprecation notice");

        let (_, level, _, message) = parse_line(unsafe_line, true);
        assert_eq!(level, LogLevel::Warn, "{unsafe_line}");
        // The whole line is kept: the prefix is part of what the JVM said.
        assert!(message.contains("sun.misc.Unsafe"), "{message}");

        // Every WARNING: line in the fixture, on stderr, is a warning.
        for line in fixture.lines().filter(|line| line.starts_with("WARNING:")) {
            let (_, level, _, _) = parse_line(line, true);
            assert_eq!(level, LogLevel::Warn, "{line}");
        }
    }

    #[test]
    fn the_jvms_own_error_prefix_still_reads_as_an_error() {
        let (_, level, _, _) = parse_line("ERROR: could not create the Java Virtual Machine", true);
        assert_eq!(level, LogLevel::Error);

        // And on stdout, where the stream says nothing.
        let (_, level, _, _) = parse_line("SEVERE: something went wrong", false);
        assert_eq!(level, LogLevel::Error);
        let (_, level, _, _) = parse_line("INFO: starting up", false);
        assert_eq!(level, LogLevel::Info);
    }

    #[test]
    fn a_stderr_line_that_declares_nothing_still_falls_back_to_the_stream() {
        // The PDH counter noise and a bare stack trace line say nothing about
        // themselves, so the stream is all there is to go on.
        let fixture = include_str!("../../tests/fixtures/log_jvm_stderr.txt");
        for line in fixture
            .lines()
            .filter(|line| line.starts_with("Failed to add PDH Counter"))
        {
            let (_, level, _, _) = parse_line(line, true);
            assert_eq!(level, LogLevel::Error, "{line}");
            let (_, level, _, _) = parse_line(line, false);
            assert_eq!(level, LogLevel::Raw, "on stdout it is unclassified: {line}");
        }

        let trace = "com.sun.jna.platform.win32.Win32Exception: The parameter is incorrect.";
        assert_eq!(parse_line(trace, true).1, LogLevel::Error);
    }

    #[test]
    fn the_first_boot_properties_complaint_is_recognised() {
        let fixture = include_str!("../../tests/fixtures/log_first_boot_properties.txt");
        let header = fixture
            .lines()
            .find(|line| line.contains("Failed to load properties"))
            .expect("the fixture has the complaint");

        // It really is an ERROR line as the server prints it.
        let (_, level, _, message) = parse_line(header, false);
        assert_eq!(level, LogLevel::Error);
        assert!(is_missing_properties_header(&message), "{message}");

        // Nothing else in the fixture is.
        for line in fixture
            .lines()
            .filter(|line| !line.contains("Failed to load properties"))
        {
            let (_, _, _, message) = parse_line(line, false);
            assert!(!is_missing_properties_header(&message), "{line}");
        }
    }

    #[test]
    fn every_frame_under_the_exception_is_a_continuation() {
        let fixture = include_str!("../../tests/fixtures/log_first_boot_properties.txt");
        let mut lines = fixture
            .lines()
            .skip_while(|line| !line.contains("NoSuchFileException"));

        // The exception class, then its five frames.
        assert!(is_exception_continuation(lines.next().unwrap()));
        for line in lines.by_ref().take(5) {
            assert!(is_exception_continuation(line), "{line}");
        }

        // The server's next real line ends the block.
        let next = lines.next().unwrap();
        assert!(next.contains("Loaded 1 recipes"));
        assert!(!is_exception_continuation(next), "{next}");
    }

    #[test]
    fn a_log4j_line_is_never_taken_for_a_stack_frame() {
        // Paper prints its early lines in the log4j layout, with no bracket to
        // give the shape away.
        assert!(!is_exception_continuation(
            "2026-08-18 12:04:12,123 main INFO  Loading libraries"
        ));
        assert!(!is_exception_continuation(
            "Starting minecraft server version 1.21.4"
        ));
        assert!(!is_exception_continuation(""));
    }

    #[test]
    fn only_the_properties_complaint_is_matched() {
        assert!(is_missing_properties_header(
            "Failed to load properties from file: server.properties"
        ));
        // Some builds print a path rather than a bare name.
        assert!(is_missing_properties_header(
            "Failed to load properties from file: ./server.properties"
        ));
        // A different file going missing is a different problem.
        assert!(!is_missing_properties_header(
            "Failed to load properties from file: bukkit.yml"
        ));
        assert!(!is_missing_properties_header("Failed to load eula.txt"));
    }

    #[test]
    fn prose_is_not_mistaken_for_a_level_prefix() {
        // Only a level word followed by a colon at the start of the line counts.
        for line in [
            "Loading libraries, please wait...",
            "warning signs are placed near spawn",
            "WARNING",
            "WARNINGS: two of them",
            "http://example.com/warning: not a level",
        ] {
            let (_, level, _, _) = parse_line(line, false);
            assert_eq!(level, LogLevel::Raw, "{line}");
        }
    }

    #[test]
    fn a_formatted_line_still_takes_its_level_from_the_format() {
        // The bracketed format is unambiguous and must not be second-guessed by
        // anything the message happens to contain.
        let (_, level, _, message) =
            parse_line("[17:15:57] [main/ERROR]: Unable to locate English counter names", true);
        assert_eq!(level, LogLevel::Error);
        assert!(message.starts_with("Unable to locate"));

        let (_, level, _, _) =
            parse_line("[17:15:57] [Server thread/INFO]: WARNING: this is the message", false);
        assert_eq!(level, LogLevel::Info, "the format wins over words in the message");
    }
}
