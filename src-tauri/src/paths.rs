//! Path construction and validation. Everything here is `PathBuf`-based and has
//! to behave on both Windows and Linux, so the rules are the strict union of
//! both platforms' restrictions.

use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};

/// Our metadata folder inside every instance directory.
pub const MSM_DIR: &str = ".msm";
pub const INSTANCE_JSON: &str = "instance.json";
pub const CONSOLE_DIR: &str = "console";

pub fn msm_dir(instance_path: &Path) -> PathBuf {
    instance_path.join(MSM_DIR)
}

pub fn instance_json_path(instance_path: &Path) -> PathBuf {
    msm_dir(instance_path).join(INSTANCE_JSON)
}

pub fn console_dir(instance_path: &Path) -> PathBuf {
    msm_dir(instance_path).join(CONSOLE_DIR)
}

pub fn eula_path(instance_path: &Path) -> PathBuf {
    instance_path.join("eula.txt")
}

pub fn server_properties_path(instance_path: &Path) -> PathBuf {
    instance_path.join("server.properties")
}

/// `mods` for mod loaders, `plugins` for the Bukkit-family servers.
pub fn content_dir(instance_path: &Path, dir_name: &str) -> PathBuf {
    instance_path.join(dir_name)
}

const RESERVED_WINDOWS_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

pub const MAX_NAME_LEN: usize = 64;

/// Validates the user-visible instance name. Display names are stored as typed;
/// only the derived folder name is sanitized.
pub fn validate_instance_name(name: &str) -> AppResult<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(AppError::InvalidName("the name cannot be empty".into()));
    }
    if trimmed.chars().count() > MAX_NAME_LEN {
        return Err(AppError::InvalidName(format!(
            "the name cannot be longer than {MAX_NAME_LEN} characters"
        )));
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err(AppError::InvalidName(
            "the name cannot contain control characters".into(),
        ));
    }
    Ok(())
}

/// Turns a display name into a folder name that is legal on Windows *and* Linux.
/// Invalid characters become `-`, runs collapse, reserved device names get a
/// suffix, and trailing dots/spaces (illegal on Windows) are trimmed.
pub fn sanitize_folder_name(name: &str) -> AppResult<String> {
    validate_instance_name(name)?;

    let mut out = String::with_capacity(name.len());
    for ch in name.trim().chars() {
        let mapped = match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '-',
            c if c.is_control() => '-',
            c if c.is_whitespace() => '-',
            c => c,
        };
        if mapped == '-' && out.ends_with('-') {
            continue;
        }
        out.push(mapped);
    }

    let out = out.trim_matches(|c: char| c == '-' || c == '.' || c.is_whitespace());
    if out.is_empty() {
        return Err(AppError::InvalidName(
            "the name contains no usable characters for a folder name".into(),
        ));
    }

    let stem_upper = out
        .split('.')
        .next()
        .unwrap_or(out)
        .to_ascii_uppercase();
    if RESERVED_WINDOWS_NAMES.contains(&stem_upper.as_str()) {
        return Ok(format!("{out}-server"));
    }

    Ok(out.to_string())
}

/// Picks a directory under `root` that does not exist yet, appending `-2`, `-3`, …
pub fn unique_dir(root: &Path, base: &str) -> PathBuf {
    let first = root.join(base);
    if !first.exists() {
        return first;
    }
    for n in 2..1000 {
        let candidate = root.join(format!("{base}-{n}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    root.join(format!("{base}-{}", uuid::Uuid::new_v4()))
}

/// Canonicalizes without the Windows `\\?\` verbatim prefix, falling back to the
/// input when the path does not exist yet.
pub fn normalize(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Compares two paths for "same location" on both platforms. Windows is
/// case-insensitive, Linux is not, so the comparison follows the host.
pub fn same_path(a: &Path, b: &Path) -> bool {
    let (a, b) = (normalize(a), normalize(b));
    if cfg!(windows) {
        a.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
    } else {
        a == b
    }
}

/// True when the directory holds no entries (a nonexistent directory counts as empty).
pub fn dir_is_empty(path: &Path) -> AppResult<bool> {
    if !path.exists() {
        return Ok(true);
    }
    let mut entries = std::fs::read_dir(path).map_err(|e| AppError::io("read folder", path, e))?;
    Ok(entries.next().is_none())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_and_overlong_names() {
        assert!(validate_instance_name("   ").is_err());
        assert!(validate_instance_name(&"x".repeat(MAX_NAME_LEN + 1)).is_err());
        assert!(validate_instance_name("Survival 1.20").is_ok());
    }

    #[test]
    fn sanitizes_windows_illegal_characters() {
        assert_eq!(sanitize_folder_name("My: Server?").unwrap(), "My-Server");
        assert_eq!(sanitize_folder_name("a/b\\c").unwrap(), "a-b-c");
        assert_eq!(sanitize_folder_name("Survival 1.20").unwrap(), "Survival-1.20");
    }

    #[test]
    fn trims_trailing_dots_and_spaces() {
        // "name." and "name " are legal on Linux but not on Windows.
        assert_eq!(sanitize_folder_name("Server...").unwrap(), "Server");
        assert_eq!(sanitize_folder_name("  Server  ").unwrap(), "Server");
    }

    #[test]
    fn escapes_reserved_windows_device_names() {
        assert_eq!(sanitize_folder_name("con").unwrap(), "con-server");
        assert_eq!(sanitize_folder_name("COM1").unwrap(), "COM1-server");
        assert_eq!(sanitize_folder_name("nul.txt").unwrap(), "nul.txt-server");
        assert_eq!(sanitize_folder_name("console").unwrap(), "console");
    }

    #[test]
    fn rejects_names_with_no_usable_characters() {
        assert!(sanitize_folder_name("///").is_err());
        assert!(sanitize_folder_name("...").is_err());
    }

    #[test]
    fn paths_are_joined_not_concatenated() {
        let root = PathBuf::from("base");
        let p = instance_json_path(&root);
        // The separator is whatever the host uses; the components are what matter.
        let parts: Vec<_> = p.components().map(|c| c.as_os_str().to_owned()).collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[1], std::ffi::OsStr::new(MSM_DIR));
        assert_eq!(parts[2], std::ffi::OsStr::new(INSTANCE_JSON));
    }

    #[test]
    fn unique_dir_avoids_existing_folders() {
        let tmp = std::env::temp_dir().join(format!("msm-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(tmp.join("srv")).unwrap();
        assert_eq!(unique_dir(&tmp, "srv"), tmp.join("srv-2"));
        assert_eq!(unique_dir(&tmp, "other"), tmp.join("other"));
        std::fs::remove_dir_all(&tmp).ok();
    }
}
