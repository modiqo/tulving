//! The OS is the clock. No daemon: `init` registers a per-user timer
//! that runs `tulving tick` every 60 seconds — launchd on macOS, a
//! systemd user timer on Linux (crontab as the fallback), Task
//! Scheduler on Windows when demand appears (docs/DESIGN.md §16).

use anyhow::{bail, Context, Result};

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn current_exe() -> Result<String> {
    Ok(std::env::current_exe()
        .context("cannot resolve tulving binary path")?
        .display()
        .to_string())
}

// ---------------------------------------------------------------- macOS

#[cfg(target_os = "macos")]
const LAUNCHD_LABEL: &str = "ai.tulving.tick";

#[cfg(target_os = "macos")]
fn plist_path() -> Result<std::path::PathBuf> {
    let home = std::env::var("HOME").context("HOME is not set")?;
    Ok(std::path::PathBuf::from(home)
        .join("Library/LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist")))
}

/// Write and load the launchd agent.
#[cfg(target_os = "macos")]
pub fn install_timer() -> Result<()> {
    let exe = current_exe()?;
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LAUNCHD_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
    <string>tick</string>
  </array>
  <key>StartInterval</key><integer>60</integer>
  <key>RunAtLoad</key><true/>
</dict>
</plist>
"#
    );
    let path = plist_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, plist)?;
    let path_str = path.display().to_string();
    // Reload if already registered; ignore the error when it was not.
    let _ = std::process::Command::new("launchctl")
        .args(["unload", &path_str])
        .output();
    let load = std::process::Command::new("launchctl")
        .args(["load", &path_str])
        .output()
        .context("launchctl not available")?;
    if !load.status.success() {
        bail!(
            "launchctl load failed: {}",
            String::from_utf8_lossy(&load.stderr)
        );
    }
    println!("✓ launchd timer installed ({LAUNCHD_LABEL}); tick runs every 60s");
    println!("  plist: {path_str}");
    Ok(())
}

/// Unload and delete the launchd agent.
#[cfg(target_os = "macos")]
pub fn remove_timer() -> Result<()> {
    let path = plist_path()?;
    let _ = std::process::Command::new("launchctl")
        .args(["unload", &path.display().to_string()])
        .output();
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    println!("✓ launchd timer removed");
    Ok(())
}

// ---------------------------------------------------------------- Linux

#[cfg(target_os = "linux")]
const UNIT_NAME: &str = "tulving-tick";

#[cfg(target_os = "linux")]
fn systemd_user_dir() -> Result<std::path::PathBuf> {
    let home = std::env::var("HOME").context("HOME is not set")?;
    Ok(std::path::PathBuf::from(home).join(".config/systemd/user"))
}

#[cfg(target_os = "linux")]
fn systemctl_user(args: &[&str]) -> Result<std::process::Output> {
    std::process::Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .context("systemctl not available")
}

/// Prefer a systemd user timer (`Persistent=true` catches up after
/// sleep); fall back to a crontab line when systemd is absent.
#[cfg(target_os = "linux")]
pub fn install_timer() -> Result<()> {
    if systemctl_user(&["--version"]).is_ok() {
        return install_systemd_timer();
    }
    install_crontab_line()
}

#[cfg(target_os = "linux")]
fn install_systemd_timer() -> Result<()> {
    let exe = current_exe()?;
    let dir = systemd_user_dir()?;
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join(format!("{UNIT_NAME}.service")),
        format!(
            "[Unit]\nDescription=tulving tick\n\n\
             [Service]\nType=oneshot\nExecStart={exe} tick\n"
        ),
    )?;
    std::fs::write(
        dir.join(format!("{UNIT_NAME}.timer")),
        "[Unit]\nDescription=tulving tick every minute\n\n\
         [Timer]\nOnCalendar=minutely\nPersistent=true\n\n\
         [Install]\nWantedBy=timers.target\n",
    )?;
    systemctl_user(&["daemon-reload"])?;
    let enable = systemctl_user(&["enable", "--now", &format!("{UNIT_NAME}.timer")])?;
    if !enable.status.success() {
        bail!(
            "systemctl enable failed: {}",
            String::from_utf8_lossy(&enable.stderr)
        );
    }
    println!("✓ systemd user timer installed ({UNIT_NAME}.timer); tick runs every 60s");
    println!("  headless servers also need: loginctl enable-linger $USER");
    Ok(())
}

#[cfg(target_os = "linux")]
fn install_crontab_line() -> Result<()> {
    let exe = current_exe()?;
    let line = format!("* * * * * {exe} tick");
    let existing = std::process::Command::new("crontab")
        .arg("-l")
        .output()
        .context("neither systemd nor crontab is available")?;
    let mut table = if existing.status.success() {
        String::from_utf8_lossy(&existing.stdout).to_string()
    } else {
        String::new() // no crontab yet
    };
    if table.contains(&line) {
        println!("✓ crontab line already present");
        return Ok(());
    }
    if !table.is_empty() && !table.ends_with('\n') {
        table.push('\n');
    }
    table.push_str(&line);
    table.push('\n');
    write_crontab(&table)?;
    println!("✓ crontab line installed: {line}");
    Ok(())
}

#[cfg(target_os = "linux")]
fn write_crontab(table: &str) -> Result<()> {
    use std::io::Write as _;
    let mut child = std::process::Command::new("crontab")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .context("cannot run crontab -")?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(table.as_bytes())?;
    }
    let status = child.wait()?;
    if !status.success() {
        bail!("crontab rejected the new table");
    }
    Ok(())
}

/// Remove whichever timer `init` installed.
#[cfg(target_os = "linux")]
pub fn remove_timer() -> Result<()> {
    let mut removed = false;
    if systemctl_user(&["--version"]).is_ok() {
        let _ = systemctl_user(&["disable", "--now", &format!("{UNIT_NAME}.timer")]);
        let dir = systemd_user_dir()?;
        for unit in [format!("{UNIT_NAME}.service"), format!("{UNIT_NAME}.timer")] {
            let path = dir.join(unit);
            if path.exists() {
                std::fs::remove_file(&path)?;
                removed = true;
            }
        }
        let _ = systemctl_user(&["daemon-reload"]);
    }
    if let Ok(existing) = std::process::Command::new("crontab").arg("-l").output() {
        if existing.status.success() {
            let table = String::from_utf8_lossy(&existing.stdout);
            let kept: String = table
                .lines()
                .filter(|l| !l.contains("tulving tick"))
                .map(|l| format!("{l}\n"))
                .collect();
            if kept != table {
                write_crontab(&kept)?;
                removed = true;
            }
        }
    }
    if removed {
        println!("✓ timer removed");
    } else {
        println!("no tulving timer was installed");
    }
    Ok(())
}

// --------------------------------------------------------------- others

/// Not supported yet; docs/DESIGN.md §16 gates Windows on demand.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn install_timer() -> Result<()> {
    bail!("tulving init supports macOS and Linux today; Windows arrives with M5")
}

/// Not supported yet.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn remove_timer() -> Result<()> {
    bail!("no timer to remove on this platform yet")
}

// ---------------------------------------------------------------- status

/// Which timer drives tick on this machine, if any. `status` prints it.
#[cfg(target_os = "macos")]
pub fn timer_status() -> Option<String> {
    let path = plist_path().ok()?;
    path.exists()
        .then(|| format!("launchd agent {LAUNCHD_LABEL} (every 60s)"))
}

/// Which timer drives tick on this machine, if any. `status` prints it.
#[cfg(target_os = "linux")]
pub fn timer_status() -> Option<String> {
    if let Ok(dir) = systemd_user_dir() {
        if dir.join(format!("{UNIT_NAME}.timer")).exists() {
            return Some(format!("systemd user timer {UNIT_NAME}.timer (every 60s)"));
        }
    }
    let out = std::process::Command::new("crontab")
        .arg("-l")
        .output()
        .ok()?;
    let table = String::from_utf8_lossy(&out.stdout);
    table
        .lines()
        .any(|l| l.contains("tulving tick"))
        .then(|| "crontab line (every 60s)".to_string())
}

/// Which timer drives tick on this machine, if any.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn timer_status() -> Option<String> {
    None
}
