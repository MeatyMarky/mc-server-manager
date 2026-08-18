//! Console capture: a bounded in-memory ring buffer plus rotated files on disk.
//!
//! During chunk generation a server prints thousands of lines a second. The ring
//! buffer keeps memory flat, the batching in `supervisor.rs` keeps the IPC
//! bridge from being flooded, and the rotated files under `.msm/console/` keep
//! the history the buffer drops.

use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{AppResult, IoContext};
use crate::logparse::{self, LogLevel, ParsedLine};
use crate::paths;

/// Lines kept in memory per instance. Roughly 1 MB of text at 200 bytes a line.
pub const RING_CAPACITY: usize = 5_000;
/// A console file is rotated once it passes this size.
pub const ROTATE_AT_BYTES: u64 = 5 * 1024 * 1024;
/// How many rotated files to keep before the oldest is deleted.
pub const KEEP_FILES: usize = 5;

/// Bounded history of what a server printed.
#[derive(Debug)]
pub struct ConsoleBuffer {
    lines: VecDeque<ParsedLine>,
    next_seq: u64,
    capacity: usize,
}

impl ConsoleBuffer {
    pub fn new() -> Self {
        Self::with_capacity(RING_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            lines: VecDeque::with_capacity(capacity.min(1024)),
            next_seq: 0,
            capacity,
        }
    }

    /// Parses and stores one line, returning the structured form to be emitted.
    pub fn push(&mut self, raw: &str, stderr: bool) -> ParsedLine {
        let (timestamp, level, thread, message) = logparse::parse_line(raw, stderr);
        let line = ParsedLine {
            seq: self.next_seq,
            captured_at: crate::db::now_rfc3339(),
            timestamp,
            level,
            thread,
            message,
            raw: raw.trim_end_matches(['\r', '\n']).to_string(),
            stderr,
        };
        self.next_seq += 1;

        if self.lines.len() == self.capacity {
            self.lines.pop_front();
        }
        self.lines.push_back(line.clone());
        line
    }

    /// A line the app itself wrote (start banner, stop notice), so the console
    /// explains what happened instead of going silent.
    pub fn push_system(&mut self, message: &str) -> ParsedLine {
        let line = ParsedLine {
            seq: self.next_seq,
            captured_at: crate::db::now_rfc3339(),
            timestamp: None,
            level: LogLevel::Info,
            thread: Some("manager".to_string()),
            message: message.to_string(),
            raw: message.to_string(),
            stderr: false,
        };
        self.next_seq += 1;
        if self.lines.len() == self.capacity {
            self.lines.pop_front();
        }
        self.lines.push_back(line.clone());
        line
    }

    /// The most recent `count` lines, oldest first.
    pub fn tail(&self, count: usize) -> Vec<ParsedLine> {
        let skip = self.lines.len().saturating_sub(count);
        self.lines.iter().skip(skip).cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Total lines seen, including ones the ring has already dropped.
    pub fn total_seen(&self) -> u64 {
        self.next_seq
    }

    pub fn clear(&mut self) {
        self.lines.clear();
    }
}

impl Default for ConsoleBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// Appends console output to `.msm/console/console.log`, rotating by size.
#[derive(Debug)]
pub struct ConsoleFile {
    dir: PathBuf,
    file: Option<std::fs::File>,
    written: u64,
}

impl ConsoleFile {
    pub fn open(instance_path: &Path) -> AppResult<Self> {
        let dir = paths::console_dir(instance_path);
        std::fs::create_dir_all(&dir).ctx("create console folder", &dir)?;

        let path = dir.join("console.log");
        let written = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ctx("open console log", &path)?;

        Ok(Self {
            dir,
            file: Some(file),
            written,
        })
    }

    pub fn write_line(&mut self, raw: &str) {
        let Some(file) = self.file.as_mut() else {
            return;
        };
        // Console capture must never take a server down: log and carry on.
        if let Err(err) = writeln!(file, "{raw}") {
            tracing::warn!(error = %err, "could not write to the console log");
            self.file = None;
            return;
        }
        self.written += raw.len() as u64 + 1;

        if self.written >= ROTATE_AT_BYTES {
            if let Err(err) = self.rotate() {
                tracing::warn!(error = %err, "could not rotate the console log");
            }
        }
    }

    fn rotate(&mut self) -> AppResult<()> {
        self.file = None;
        let current = self.dir.join("console.log");
        let stamped = self.dir.join(format!(
            "console-{}.log",
            crate::db::now_rfc3339().replace([':', '-'], "").replace('T', "-")
        ));
        std::fs::rename(&current, &stamped).ctx("rotate console log", &current)?;

        prune_rotated(&self.dir, KEEP_FILES)?;

        self.file = Some(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&current)
                .ctx("open console log", &current)?,
        );
        self.written = 0;
        Ok(())
    }
}

/// Keeps the newest `keep` rotated files and deletes the rest.
pub fn prune_rotated(dir: &Path, keep: usize) -> AppResult<()> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    let mut rotated: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with("console-") && name.ends_with(".log"))
                .unwrap_or(false)
        })
        .collect();

    rotated.sort();
    while rotated.len() > keep {
        let oldest = rotated.remove(0);
        let _ = std::fs::remove_file(oldest);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_keeps_the_newest_lines_and_flat_memory() {
        let mut buffer = ConsoleBuffer::with_capacity(100);
        for index in 0..1_000 {
            buffer.push(&format!("[12:00:00] [Server thread/INFO]: line {index}"), false);
        }
        assert_eq!(buffer.len(), 100, "the ring is bounded");
        assert_eq!(buffer.total_seen(), 1_000);

        let tail = buffer.tail(5);
        assert_eq!(tail.len(), 5);
        assert_eq!(tail.last().unwrap().message, "line 999");
        assert_eq!(tail.first().unwrap().message, "line 995");
    }

    #[test]
    fn sequence_numbers_survive_eviction() {
        let mut buffer = ConsoleBuffer::with_capacity(3);
        for index in 0..10 {
            buffer.push(&format!("line {index}"), false);
        }
        let seqs: Vec<u64> = buffer.tail(3).iter().map(|line| line.seq).collect();
        assert_eq!(seqs, vec![7, 8, 9], "seq is monotonic, not an index");
    }

    #[test]
    fn tail_asks_for_more_than_exists_without_panicking() {
        let mut buffer = ConsoleBuffer::new();
        assert!(buffer.tail(500).is_empty());
        buffer.push("one line", false);
        assert_eq!(buffer.tail(500).len(), 1);
    }

    #[test]
    fn lines_keep_their_structure_and_their_raw_text() {
        let mut buffer = ConsoleBuffer::new();
        let line = buffer.push("[12:34:56] [Server thread/WARN]: Can't keep up!\r\n", false);
        assert_eq!(line.level, LogLevel::Warn);
        assert_eq!(line.thread.as_deref(), Some("Server thread"));
        assert_eq!(line.message, "Can't keep up!");
        assert_eq!(line.raw, "[12:34:56] [Server thread/WARN]: Can't keep up!");
        assert!(!line.stderr);
    }

    #[test]
    fn system_lines_are_marked_as_coming_from_the_manager() {
        let mut buffer = ConsoleBuffer::new();
        let line = buffer.push_system("Starting server (java 21)");
        assert_eq!(line.thread.as_deref(), Some("manager"));
        assert_eq!(line.level, LogLevel::Info);
    }

    #[test]
    fn console_files_rotate_and_prune() {
        let dir = tempfile::tempdir().unwrap();
        let console = paths::console_dir(dir.path());
        std::fs::create_dir_all(&console).unwrap();

        for index in 0..8 {
            std::fs::write(
                console.join(format!("console-2026081{index}-000000Z.log")),
                b"old",
            )
            .unwrap();
        }
        prune_rotated(&console, 5).unwrap();

        let remaining: Vec<String> = std::fs::read_dir(&console)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(remaining.len(), 5, "only the newest files survive");
        assert!(remaining.iter().any(|name| name.contains("20260817")));
        assert!(!remaining.iter().any(|name| name.contains("20260810")));
    }

    #[test]
    fn writing_creates_the_console_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = ConsoleFile::open(dir.path()).unwrap();
        file.write_line("[12:00:00] [Server thread/INFO]: hello");
        drop(file);

        let contents =
            std::fs::read_to_string(paths::console_dir(dir.path()).join("console.log")).unwrap();
        assert!(contents.contains("hello"));
    }
}
