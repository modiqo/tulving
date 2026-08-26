//! Self-update against GitHub releases. `--check` reports JSON so a
//! harness (Play, a skill, a script) can offer the update; the install
//! path replaces the binary via a fresh inode, never in place — copying
//! over a running binary stales the macOS code-signature cache and the
//! kernel kills it with SIGKILL.

use anyhow::{bail, Context, Result};

const REPO: &str = "modiqo/tulving";

/// Check for, and unless `check_only`, install the latest release.
pub fn run(check_only: bool) -> Result<()> {
    let installed = env!("CARGO_PKG_VERSION");
    let latest_tag = latest_tag()?;
    let latest = latest_tag.trim_start_matches('v');
    let available = latest != installed;

    if check_only {
        println!(
            "{}",
            serde_json::json!({
                "installed": installed,
                "latest": latest,
                "update_available": available,
            })
        );
        return Ok(());
    }
    if !available {
        println!("tulving {installed} is current");
        return Ok(());
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
