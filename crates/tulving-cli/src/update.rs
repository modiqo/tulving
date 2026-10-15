//! Self-update against GitHub releases. `--check` reports JSON so a
//! harness (Play, a skill, a script) can offer the update; the install
//! path replaces the binary via a fresh inode, never in place — copying
//! over a running binary stales the macOS code-signature cache and the
//! kernel kills it with SIGKILL.

use anyhow::{bail, Context, Result};

const REPO: &str = "modiqo/tulving";

/// How the latest release compares with the running binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    /// The latest release is newer: install it.
    Available,
    /// The installed version is the latest release.
    Current,
    /// The installed version is newer than the latest release; never downgrade.
    InstalledNewer,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Available => "available",
            Status::Current => "current",
            Status::InstalledNewer => "installed_newer",
        }
    }
}

/// Check for, and unless `check_only`, install the latest release.
/// Only a strictly newer release counts as an update.
pub fn run(check_only: bool) -> Result<()> {
    let installed = env!("CARGO_PKG_VERSION");
    let latest_tag = latest_tag()?;
    let latest = latest_tag.trim_start_matches('v');
    let status = compare(installed, latest)?;

    if check_only {
        println!(
            "{}",
            serde_json::json!({
                "installed": installed,
                "latest": latest,
                "update_available": status == Status::Available,
                "status": status.as_str(),
            })
        );
        return Ok(());
    }
    match status {
        Status::Available => {}
        Status::Current => {
            println!("tulving {installed} is current");
            return Ok(());
        }
        Status::InstalledNewer => {
            println!(
                "tulving {installed} is newer than the latest release {latest}; not downgrading"
            );
            return Ok(());
        }
    }

    let exe = std::env::current_exe().context("cannot resolve the running binary")?;
    let exe_text = exe.display().to_string();
    if exe_text.contains("/Cellar/") || exe_text.contains("linuxbrew") {
        println!("This tulving is managed by Homebrew; updating via brew.");
        let status = std::process::Command::new("brew")
            .args(["upgrade", "modiqo/tap/tulving"])
            .status()
            .context("brew is not available")?;
        if !status.success() {
            bail!("brew upgrade failed");
        }
        return Ok(());
    }

    let target = current_target()?;
    let url = format!(
        "https://github.com/{REPO}/releases/download/{latest_tag}/tulving-{latest_tag}-{target}.tar.gz"
    );
    let staging = tempfile_dir()?;
    let archive = staging.join("tulving.tar.gz");
    curl(&url, &archive)?;
    let status = std::process::Command::new("tar")
        .args(["xzf", &archive.display().to_string(), "-C"])
        .arg(&staging)
        .status()
        .context("tar is not available")?;
    if !status.success() {
        bail!("cannot extract the release archive");
    }

    // Stage beside the destination, then rename: same filesystem, fresh
    // inode, and the running process keeps its own (now unlinked) image.
    let staged = exe.with_extension("update-staged");
    std::fs::copy(staging.join("tulving"), &staged)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&staged, &exe)?;
    let _ = std::fs::remove_dir_all(&staging);
    println!(
        "✓ updated tulving {installed} -> {latest} ({})",
        exe.display()
    );
    Ok(())
}

fn latest_tag() -> Result<String> {
    let out = std::process::Command::new("curl")
        .args([
            "-fsSL",
            &format!("https://api.github.com/repos/{REPO}/releases/latest"),
        ])
        .output()
        .context("curl is not available")?;
    if !out.status.success() {
        bail!("cannot reach GitHub releases");
    }
    let body: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    body.get("tag_name")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .context("release response has no tag_name")
}

fn curl(url: &str, dest: &std::path::Path) -> Result<()> {
    let status = std::process::Command::new("curl")
        .args(["-fsSL", url, "-o", &dest.display().to_string()])
        .status()
        .context("curl is not available")?;
    if !status.success() {
        bail!("download failed: {url}");
    }
    Ok(())
}

fn current_target() -> Result<&'static str> {
    Ok(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        (os, arch) => bail!("no prebuilt binary for {os}/{arch}; build from source"),
    })
}

fn tempfile_dir() -> Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join(format!("tulving-update-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Compare the installed version with the latest release tag (without `v`).
fn compare(installed: &str, latest: &str) -> Result<Status> {
    let installed_version = parse_version(installed)
        .with_context(|| format!("cannot parse the installed version {installed:?}"))?;
    let latest_version = parse_version(latest)
        .with_context(|| format!("cannot parse the latest release version {latest:?}"))?;
    Ok(match latest_version.cmp(&installed_version) {
        std::cmp::Ordering::Greater => Status::Available,
        std::cmp::Ordering::Equal => Status::Current,
        std::cmp::Ordering::Less => Status::InstalledNewer,
    })
}

/// Parse a `MAJOR.MINOR.PATCH` release version. Pre-release and build
/// suffixes are rejected: releases are tagged as plain `vX.Y.Z`.
fn parse_version(text: &str) -> Result<(u64, u64, u64)> {
    let mut numbers = Vec::with_capacity(3);
    for part in text.split('.') {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            bail!("expected MAJOR.MINOR.PATCH, got {text:?}");
        }
        numbers.push(
            part.parse::<u64>()
                .with_context(|| format!("version component {part:?} is out of range"))?,
        );
    }
    match numbers.as_slice() {
        [major, minor, patch] => Ok((*major, *minor, *patch)),
        _ => bail!("expected MAJOR.MINOR.PATCH, got {text:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{compare, Status};

    #[test]
    fn newer_release_is_available() {
        assert_eq!(compare("0.1.5", "0.1.6").ok(), Some(Status::Available));
        assert_eq!(compare("0.1.9", "0.1.10").ok(), Some(Status::Available));
        assert_eq!(compare("0.9.9", "1.0.0").ok(), Some(Status::Available));
    }

    #[test]
    fn equal_release_is_current() {
        assert_eq!(compare("0.1.5", "0.1.5").ok(), Some(Status::Current));
    }

    #[test]
    fn older_release_is_not_a_downgrade() {
        assert_eq!(compare("0.1.6", "0.1.5").ok(), Some(Status::InstalledNewer));
        assert_eq!(
            compare("0.1.10", "0.1.9").ok(),
            Some(Status::InstalledNewer)
        );
        assert_eq!(compare("1.0.0", "0.9.9").ok(), Some(Status::InstalledNewer));
    }

    #[test]
    fn unparseable_version_is_an_error() {
        for (installed, latest) in [
            ("0.1.5", "nightly"),
            ("0.1.5", "0.1"),
            ("0.1.5", "0.1.6.1"),
            ("0.1.5", "0.1.6-rc.1"),
            ("0.1.5", ""),
            ("garbage", "0.1.5"),
        ] {
            assert!(
                compare(installed, latest).is_err(),
                "{installed} vs {latest} should not parse"
            );
        }
        let message = compare("0.1.5", "nightly")
            .err()
            .map(|e| format!("{e:#}"))
            .unwrap_or_default();
        assert!(
            message.contains("cannot parse the latest release version \"nightly\""),
            "{message}"
        );
    }
}
