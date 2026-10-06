//! Works out the version the app reports, and embeds the icon and version
//! information into the Windows executable, so Explorer, the taskbar and the
//! file's Properties dialog show them.
//!
//! The version is the workspace's (`0.2.0`) in a release build, which the
//! release workflow marks with `MT_RELEASE=1`. Any other build adds the commit
//! it was built from (`0.2.0+3f2a1c9`), so a bug report from a CI build or a
//! source checkout says exactly what was running. Without git (a source
//! archive), it is the workspace version alone.

use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon/miso-terminal.ico");
    println!("cargo:rerun-if-env-changed=MT_RELEASE");
    let version = version();
    println!("cargo:rustc-env=MT_VERSION={version}");

    // Build scripts run on the host; only embed when the host can (Windows) and
    // the target is Windows.
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon/miso-terminal.ico")
            .set("ProductName", "MISO Terminal")
            .set("FileDescription", "MISO Terminal")
            .set("ProductVersion", &version)
            .set(
                "LegalCopyright",
                "Copyright (C) 2026 Aren Desai. Licensed under the AGPL-3.0-or-later.",
            );
        // A missing resource compiler (rc.exe) should cost the icon, not the build.
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the Windows icon: {e}");
        }
    }
}

fn version() -> String {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    if std::env::var("MT_RELEASE").as_deref() == Ok("1") {
        return version;
    }
    match commit() {
        Some(commit) => format!("{version}+{commit}"),
        None => version,
    }
}

/// The checked-out commit, abbreviated, watching what moves when it changes.
fn commit() -> Option<String> {
    let git_dir = git(&["rev-parse", "--absolute-git-dir"])?;
    // Branches live in the common directory, which a worktree shares.
    let common = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .unwrap_or_else(|| git_dir.clone());
    let (git_dir, common) = (Path::new(&git_dir), Path::new(&common));
    // HEAD names the branch (or holds the commit, when detached); the branch's
    // own file, or packed-refs, holds where the branch points.
    let mut watch = vec![git_dir.join("HEAD"), common.join("packed-refs")];
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watch.push(common.join(branch));
    }
    for path in watch.iter().filter(|p| p.exists()) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    git(&["rev-parse", "--short=7", "HEAD"])
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let text = text.trim();
    (out.status.success() && !text.is_empty()).then(|| text.to_string())
}
