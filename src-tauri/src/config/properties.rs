//! A real `.properties` reader and writer.
//!
//! `server.properties` is a Java properties file, and Java's rules are not
//! `split('=')`: `:` separates too, whitespace can separate, backslashes escape
//! separators and whitespace, `\uXXXX` encodes any character, and a trailing
//! backslash continues onto the next line.
//!
//! The document keeps every physical line exactly as it was read. Untouched
//! entries are written back byte for byte, so comments, ordering, blank lines
//! and the keys plugins and forks invent all survive a round trip.

use std::collections::BTreeMap;

/// One logical entry: a comment, a blank line, or a key/value pair (which may
/// span several physical lines through continuations).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The exact source text, including its line ending. Written back verbatim
    /// unless the value was changed.
    pub raw: String,
    pub kind: EntryKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKind {
    Comment,
    Blank,
    Pair {
        key: String,
        value: String,
        /// The separator as written, so a rewritten line keeps the file's style.
        separator: String,
    },
}

/// A parsed properties document that can be edited and written back.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Properties {
    entries: Vec<Entry>,
    /// Line ending used by the file, for lines this app adds.
    newline: String,
}

impl Properties {
    /// Parses a document, preserving everything needed to reproduce it exactly.
    pub fn parse(text: &str) -> Self {
        let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let mut entries = Vec::new();
        let mut cursor = 0usize;
        let bytes: Vec<&str> = split_keep_endings(text);

        while cursor < bytes.len() {
            let line = bytes[cursor];
            let trimmed = line.trim_start();

            if trimmed.trim_end().is_empty() {
                entries.push(Entry {
                    raw: line.to_string(),
                    kind: EntryKind::Blank,
                });
                cursor += 1;
                continue;
            }

            if trimmed.starts_with('#') || trimmed.starts_with('!') {
                entries.push(Entry {
                    raw: line.to_string(),
                    kind: EntryKind::Comment,
                });
                cursor += 1;
                continue;
            }

            // A logical line continues while the physical line ends with an odd
            // number of backslashes.
            let mut raw = String::from(line);
            let mut logical = strip_newline(line).to_string();
            while ends_with_continuation(&logical) {
                logical.truncate(logical.len() - 1);
                cursor += 1;
                let Some(next) = bytes.get(cursor) else {
                    break;
                };
                raw.push_str(next);
                logical.push_str(strip_newline(next).trim_start());
            }
            cursor += 1;

            match split_key_value(&logical) {
                Some((key, separator, value)) => entries.push(Entry {
                    raw,
                    kind: EntryKind::Pair {
                        key: unescape(&key),
                        value: unescape(&value),
                        separator,
                    },
                }),
                // A line with no separator at all is a key with an empty value.
                None => entries.push(Entry {
                    raw,
                    kind: EntryKind::Pair {
                        key: unescape(logical.trim()),
                        value: String::new(),
                        separator: "=".to_string(),
                    },
                }),
            }
        }

        Self {
            entries,
            newline: newline.to_string(),
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.iter().rev().find_map(|entry| match &entry.kind {
            EntryKind::Pair { key: k, value, .. } if k == key => Some(value.as_str()),
            _ => None,
        })
    }

    /// Keys in file order. Later duplicates win in Java, and so do they here.
    pub fn keys(&self) -> Vec<String> {
        let mut seen = Vec::new();
        for entry in &self.entries {
            if let EntryKind::Pair { key, .. } = &entry.kind {
                if !seen.contains(key) {
                    seen.push(key.clone());
                }
            }
        }
        seen
    }

    pub fn as_map(&self) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        for entry in &self.entries {
            if let EntryKind::Pair { key, value, .. } = &entry.kind {
                map.insert(key.clone(), value.clone());
            }
        }
        map
    }

    /// Sets a value, rewriting only that line. A key that does not exist yet is
    /// appended at the end.
    pub fn set(&mut self, key: &str, value: &str) {
        let newline = self.newline.clone();
        let mut updated = false;

        for entry in self.entries.iter_mut().rev() {
            let EntryKind::Pair {
                key: existing,
                value: current,
                separator,
            } = &mut entry.kind
            else {
                continue;
            };
            if existing != key {
                continue;
            }
            if current != value {
                *current = value.to_string();
                entry.raw = format!(
                    "{}{}{}{}",
                    escape_key(key),
                    separator,
                    escape_value(value),
                    line_ending(&entry.raw, &newline)
                );
            }
            updated = true;
            break;
        }

        if !updated {
            self.entries.push(Entry {
                raw: format!("{}={}{}", escape_key(key), escape_value(value), newline),
                kind: EntryKind::Pair {
                    key: key.to_string(),
                    value: value.to_string(),
                    separator: "=".to_string(),
                },
            });
        }
    }

    /// Applies several changes, returning the keys that actually changed.
    pub fn apply(&mut self, changes: &BTreeMap<String, String>) -> Vec<String> {
        let mut changed = Vec::new();
        for (key, value) in changes {
            if self.get(key) != Some(value.as_str()) {
                self.set(key, value);
                changed.push(key.clone());
            }
        }
        changed
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
}

/// Renders the document. Entries never touched keep their original bytes.
impl std::fmt::Display for Properties {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for entry in &self.entries {
            formatter.write_str(&entry.raw)?;
        }
        Ok(())
    }
}

/// Splits into physical lines, keeping the line endings attached.
fn split_keep_endings(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (index, ch) in text.char_indices() {
        if ch == '\n' {
            lines.push(&text[start..index + 1]);
            start = index + 1;
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

fn strip_newline(line: &str) -> &str {
    line.trim_end_matches('\n').trim_end_matches('\r')
}

fn line_ending(raw: &str, fallback: &str) -> String {
    if raw.ends_with("\r\n") {
        "\r\n".to_string()
    } else if raw.ends_with('\n') {
        "\n".to_string()
    } else {
        fallback.to_string()
    }
}

/// True when the line ends with an odd number of backslashes.
fn ends_with_continuation(line: &str) -> bool {
    line.chars().rev().take_while(|c| *c == '\\').count() % 2 == 1
}

/// Finds the key/value split the way Java does: the first unescaped `=`, `:`,
/// or run of whitespace.
fn split_key_value(logical: &str) -> Option<(String, String, String)> {
    let trimmed = logical.trim_start();
    let chars: Vec<char> = trimmed.chars().collect();
    let mut index = 0;
    let mut escaped = false;

    while index < chars.len() {
        let ch = chars[index];
        if escaped {
            escaped = false;
            index += 1;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '=' | ':' => break,
            c if c.is_whitespace() => break,
            _ => {}
        }
        index += 1;
    }

    if index >= chars.len() {
        return None;
    }

    let key: String = chars[..index].iter().collect();

    // Separator: optional whitespace, at most one = or :, optional whitespace.
    let mut separator = String::new();
    let mut cursor = index;
    while cursor < chars.len() && chars[cursor].is_whitespace() && chars[cursor] != '\n' {
        separator.push(chars[cursor]);
        cursor += 1;
    }
    if cursor < chars.len() && (chars[cursor] == '=' || chars[cursor] == ':') {
        separator.push(chars[cursor]);
        cursor += 1;
        while cursor < chars.len() && chars[cursor].is_whitespace() && chars[cursor] != '\n' {
            separator.push(chars[cursor]);
            cursor += 1;
        }
    }

    let value: String = chars[cursor..].iter().collect();
    Some((key, separator, value))
}

/// Java's escape rules, including `\uXXXX`.
pub fn unescape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('f') => out.push('\u{c}'),
            Some('u') => {
                let digits: String = (0..4).filter_map(|_| chars.next()).collect();
                match u32::from_str_radix(&digits, 16).ok().and_then(char::from_u32) {
                    Some(decoded) => out.push(decoded),
                    // Not a valid escape: keep it verbatim rather than losing it.
                    None => {
                        out.push_str("\\u");
                        out.push_str(&digits);
                    }
                }
            }
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// Escapes a value for writing. Non-ASCII characters are written as themselves:
/// Minecraft reads and writes `server.properties` as UTF-8, and escaping a
/// MOTD's accented characters would only make the file harder to read.
pub fn escape_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for (index, ch) in value.chars().enumerate() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{c}' => out.push_str("\\f"),
            // Leading whitespace would be swallowed on the next read.
            ' ' if index == 0 => out.push_str("\\ "),
            '#' | '!' if index == 0 => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

/// Keys additionally escape the separator characters.
pub fn escape_key(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    for ch in key.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '=' => out.push_str("\\="),
            ':' => out.push_str("\\:"),
            ' ' => out.push_str("\\ "),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "#Minecraft server properties\n\
                          #Mon Aug 18 12:00:00 UTC 2026\n\
                          allow-flight=false\n\
                          difficulty=easy\n\
                          motd=A Minecraft Server\n\
                          server-port=25565\n";

    #[test]
    fn reads_plain_pairs() {
        let properties = Properties::parse(SAMPLE);
        assert_eq!(properties.get("difficulty"), Some("easy"));
        assert_eq!(properties.get("server-port"), Some("25565"));
        assert_eq!(properties.get("missing"), None);
        assert_eq!(properties.keys().len(), 4);
    }

    #[test]
    fn round_trips_byte_for_byte() {
        let properties = Properties::parse(SAMPLE);
        assert_eq!(properties.to_string(), SAMPLE);
    }

    #[test]
    fn round_trips_a_file_with_every_awkward_feature() {
        // Colon separators, whitespace separators, escapes, continuations,
        // CRLF, a bang comment, blank lines and a key with no value.
        let awkward = "#comment\r\n\
                       !also a comment\r\n\
                       \r\n\
                       colon:separated\r\n\
                       spaced   value here\r\n\
                       escaped\\:key=value\r\n\
                       motd=Hello \\u010D\\u0161\\u017E\r\n\
                       continued=first \\\r\n\
                       second\r\n\
                       empty=\r\n\
                       no-separator\r\n";
        let properties = Properties::parse(awkward);
        assert_eq!(properties.to_string(), awkward, "byte-identical round trip");

        assert_eq!(properties.get("colon"), Some("separated"));
        assert_eq!(properties.get("spaced"), Some("value here"));
        assert_eq!(properties.get("escaped:key"), Some("value"));
        assert_eq!(properties.get("motd"), Some("Hello čšž"));
        assert_eq!(properties.get("continued"), Some("first second"));
        assert_eq!(properties.get("empty"), Some(""));
        assert_eq!(properties.get("no-separator"), Some(""));
    }

    #[test]
    fn unescapes_java_escapes() {
        assert_eq!(unescape(r"a\tb"), "a\tb");
        assert_eq!(unescape(r"a\nb"), "a\nb");
        assert_eq!(unescape(r"a\\b"), r"a\b");
        assert_eq!(unescape(r"\u010D\u0161\u017E"), "čšž");
        assert_eq!(unescape(r"\:\="), ":=");
        // A malformed escape is kept rather than silently dropped.
        assert_eq!(unescape(r"\uZZZZ"), r"\uZZZZ");
    }

    #[test]
    fn writing_only_rewrites_the_changed_line() {
        let mut properties = Properties::parse(SAMPLE);
        properties.set("difficulty", "hard");
        let rendered = properties.to_string();

        assert!(rendered.contains("difficulty=hard"));
        assert!(rendered.starts_with("#Minecraft server properties\n"), "comments stay");
        assert!(rendered.contains("motd=A Minecraft Server\n"), "other lines untouched");
        assert_eq!(rendered.lines().count(), SAMPLE.lines().count());
    }

    #[test]
    fn setting_a_value_to_what_it_already_is_changes_nothing() {
        let mut properties = Properties::parse(SAMPLE);
        properties.set("difficulty", "easy");
        assert_eq!(properties.to_string(), SAMPLE);
    }

    #[test]
    fn a_new_key_is_appended_with_the_files_line_ending() {
        let crlf = "a=1\r\nb=2\r\n";
        let mut properties = Properties::parse(crlf);
        properties.set("c", "3");
        assert_eq!(properties.to_string(), "a=1\r\nb=2\r\nc=3\r\n");
    }

    #[test]
    fn non_ascii_survives_read_edit_write() {
        let file = "motd=Server\nlevel-name=world\n";
        let mut properties = Properties::parse(file);
        properties.set("motd", "Čajovna — žíznivý šnek");

        let rendered = properties.to_string();
        assert!(rendered.contains("motd=Čajovna — žíznivý šnek"));

        // And reading it back gives exactly what was set.
        let reparsed = Properties::parse(&rendered);
        assert_eq!(reparsed.get("motd"), Some("Čajovna — žíznivý šnek"));
        assert_eq!(reparsed.get("level-name"), Some("world"));
    }

    #[test]
    fn escaped_unicode_input_is_decoded_and_rewritten_as_utf8() {
        // Java's Properties.store writes \uXXXX; reading must decode it, and a
        // later edit writes plain UTF-8, which Minecraft also reads.
        let file = "motd=\\u010Dau\n";
        let mut properties = Properties::parse(file);
        assert_eq!(properties.get("motd"), Some("čau"));

        // Untouched: the original escaped form is preserved exactly.
        assert_eq!(properties.to_string(), file);

        properties.set("motd", "čau světe");
        assert!(properties.to_string().contains("motd=čau světe"));
    }

    #[test]
    fn values_that_would_confuse_a_reader_are_escaped_on_write() {
        let mut properties = Properties::parse("a=1\n");
        properties.set("a", " leading space");
        properties.set("b", "line\nbreak");
        properties.set("c", r"back\slash");

        let rendered = properties.to_string();
        assert!(rendered.contains(r"a=\ leading space"));
        assert!(rendered.contains(r"b=line\nbreak"));
        assert!(rendered.contains(r"c=back\\slash"));

        let reparsed = Properties::parse(&rendered);
        assert_eq!(reparsed.get("a"), Some(" leading space"));
        assert_eq!(reparsed.get("b"), Some("line\nbreak"));
        assert_eq!(reparsed.get("c"), Some(r"back\slash"));
    }

    #[test]
    fn duplicate_keys_resolve_the_way_java_does() {
        let properties = Properties::parse("key=first\nkey=second\n");
        assert_eq!(properties.get("key"), Some("second"), "the last one wins");
        assert_eq!(properties.keys(), vec!["key"]);
    }

    #[test]
    fn apply_reports_only_real_changes() {
        let mut properties = Properties::parse(SAMPLE);
        let changes = BTreeMap::from([
            ("difficulty".to_string(), "hard".to_string()),
            ("motd".to_string(), "A Minecraft Server".to_string()),
            ("new-key".to_string(), "value".to_string()),
        ]);

        let mut changed = properties.apply(&changes);
        changed.sort();
        assert_eq!(changed, vec!["difficulty", "new-key"]);
    }

    #[test]
    fn a_file_without_a_trailing_newline_round_trips() {
        let text = "a=1\nb=2";
        assert_eq!(Properties::parse(text).to_string(), text);
    }

    #[test]
    fn an_empty_file_is_valid() {
        let properties = Properties::parse("");
        assert_eq!(properties.to_string(), "");
        assert!(properties.keys().is_empty());
    }

    #[test]
    fn unknown_and_plugin_keys_survive_an_unrelated_edit() {
        let file = "#comment\nmotd=hi\npaper.custom-key=42\nsome_fork_setting:yes\n";
        let mut properties = Properties::parse(file);
        properties.set("motd", "bye");

        let rendered = properties.to_string();
        assert!(rendered.contains("paper.custom-key=42"));
        assert!(rendered.contains("some_fork_setting:yes"));
        assert_eq!(
            Properties::parse(&rendered).get("some_fork_setting"),
            Some("yes")
        );
    }
}
