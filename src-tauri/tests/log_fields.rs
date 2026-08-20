//! Every log line that names an instance also carries its id.
//!
//! Three servers called "idk" produce three identical `instance=idk` lines, and
//! working out which one crashed means guessing from timestamps. The name is
//! what a person recognises and the id is what makes it unambiguous, so the
//! lines carry both — checked here rather than remembered.

use std::path::{Path, PathBuf};

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(rust_files(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
    found
}

#[test]
fn a_named_instance_is_always_logged_with_its_id() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();

    for file in rust_files(&src) {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();

        for (index, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            // A tracing field naming an instance: `instance = %name,`.
            if !(trimmed.starts_with("instance = %") || trimmed.starts_with("instance = ")) {
                continue;
            }
            // The id may sit on the same line or the next one, and either
            // spelling of the value is fine.
            let next = lines.get(index + 1).map(|line| line.trim()).unwrap_or("");
            if trimmed.contains("instance_id") || next.contains("instance_id") {
                continue;
            }
            offenders.push(format!(
                "{}:{} — {trimmed}",
                file.file_name().unwrap_or_default().to_string_lossy(),
                index + 1
            ));
        }
    }

    assert!(
        offenders.is_empty(),
        "these log lines name an instance without its id:\n{}",
        offenders.join("\n")
    );
}
