use std::process::Command;

fn main() {
    embed_git_sha();
    tauri_build::build()
}

/// Stamps the commit into the binary so a bug report can name the exact build.
///
/// A source tarball with no `.git` still has to compile, so a missing git or a
/// missing repository is "unknown", never a build failure.
fn embed_git_sha() {
    let sha = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|sha| sha.trim().to_string())
        .filter(|sha| !sha.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| !out.stdout.is_empty())
        .unwrap_or(false);

    println!(
        "cargo:rustc-env=MSM_GIT_SHA={sha}{}",
        if dirty { "-modified" } else { "" }
    );

    // Rebuild when the checkout moves, so the stamp cannot go stale.
    for path in ["../.git/HEAD", "../.git/refs/heads"] {
        if std::path::Path::new(path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}
