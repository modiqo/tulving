//! The tulving CLI. Every verb answers a question a person actually
//! asks a cron with a memory: keep doing this (`every`), what do I have
//! (`list`), has anything changed (`changed`), what happened (`digest`),
//! why does this run (`why`), run it now (`now`), stop this (`stop`),
//! quiet it for a while (`snooze`), is this thing even on (`status`).

mod platform;
mod update;

use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use tulving_core::{cadence, ops, Envelope, Ledger, Schedule, ScheduleSpec};

#[derive(Parser)]
#[command(
    name = "tulving",
    version,
    about = "A tiny cron with a memory: schedule any command, keep every result, recall on a timescale."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

// One Command is parsed once per process; variant size imbalance is free.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
enum Command {
    /// Keep doing this: tulving every morning --why "pricing watch" -- <cmd>
    ///
    /// The command runs unattended, without a terminal: anything that asks a
    /// question will fail. Pass the tool's own non-interactive flag, for
    /// example `rote play run <play> --yes`.
    #[command(visible_alias = "create")]
    Every {
        /// Plain-words cadence: morning, 15m, "weekdays 7:30", "monday 9am"
        #[arg(required = true, num_args = 1..)]
        cadence: Vec<String>,
        /// Why this schedule exists (stored beside the command)
        #[arg(long)]
        why: Option<String>,
        /// Stop after N runs
        #[arg(long)]
        max_runs: Option<i64>,
        /// Stop after a duration: 2w, 14d, 6h
        #[arg(long = "for")]
        for_: Option<String>,
        /// Flag runs whose result differs from the previous run;
        /// optional JSON pointer scopes the comparison (e.g. /plans/0/price)
        #[arg(long, num_args = 0..=1, default_missing_value = "*")]
        on_change: Option<String>,
        /// Set-diff an array result by this JSON pointer per item
        /// (e.g. /name); deltas become {added, removed, changed} and the
        /// ledger stores deltas instead of snapshots after the first run
        #[arg(long)]
        key: Option<String>,
        /// Retire when this jq predicate over the result is true
        /// (e.g. '.state == "MERGED"'; `prev` names the previous result)
        #[arg(long)]
        until: Option<String>,
        /// Run the notifier when this jq predicate over the result is true
        /// (e.g. '.dau < prev.dau * 0.95')
        #[arg(long)]
        on: Option<String>,
        /// Notifier command, split on whitespace (default: `notify` in
        /// ~/.tulving/config.toml); receives the envelope JSON on stdin
        #[arg(long)]
        notify: Option<String>,
        /// Tag for recall filtering; repeatable
        #[arg(long)]
        tag: Vec<String>,
        /// The command, after `--`
        #[arg(last = true, required = true)]
        cmd: Vec<String>,
    },
    /// Crystallize from JSON on stdin (what harness shims call): tulving add -
    Add {
        /// Must be "-" (read the spec from stdin)
        input: String,
        /// Validate and show what would be created, without writing it
        #[arg(long)]
        dry_run: bool,
    },
    /// What do I have running?
    #[command(visible_alias = "ls")]
    List {
        /// Include retired schedules
        #[arg(long)]
        all: bool,
    },
    /// Has anything changed?
    Changed {
        /// How far back to look (default: 24h)
        #[arg(long, default_value = "24h")]
        since: String,
    },
    /// What happened? A readable rollup of the ledger
    Digest {
        /// How far back to look (default: today)
        #[arg(long, default_value = "today")]
        since: String,
    },
    /// Why does this run? Add text to set the reason
    Why {
        /// Schedule id
        id: String,
        /// New reason (omit to show the current one)
        text: Vec<String>,
    },
    /// Run one schedule now, regardless of its cadence
    #[command(visible_alias = "run")]
    Now {
        /// Schedule id
        id: String,
    },
    /// Stop schedules; history stays in the ledger
    #[command(visible_alias = "retire")]
    Stop {
        /// Schedule id (omit with --all)
        id: Option<String>,
        /// Stop every active schedule
        #[arg(long)]
        all: bool,
    },
    /// Quiet schedules for a while without stopping them:
    /// tulving snooze <id> 2d, or tulving snooze --all 1w
    Snooze {
        /// Schedule id and duration (2d, 1w, 6h), in either order
        #[arg(num_args = 1..=2, required = true)]
        args: Vec<String>,
        /// Snooze every active schedule
        #[arg(long)]
        all: bool,
    },
    /// Is this thing even on? Timer, ledger, next run, recent misses
    Status,
    /// Read the ledger; JSON lines out (the agent surface)
    Recall {
        /// 'yesterday', 'today', '6h', '2d', or RFC 3339
        #[arg(long, default_value = "24h")]
        since: String,
        /// Only runs whose result moved
        #[arg(long)]
        changed: bool,
        /// Only failed runs
        #[arg(long)]
        failed: bool,
        /// Limit to one schedule id
        #[arg(long)]
        schedule: Option<String>,
    },
    /// Run whatever is due; called by the OS timer every minute
    Tick,
    /// Register the OS timer (launchd/systemd/crontab) that drives tick
    Init,
    /// Remove the OS timer
    Uninit,
    /// Safe backup of the ledger (VACUUM INTO)
    Export {
        /// Destination file
        path: String,
    },
    /// Serve MCP over stdio (stateless; spec 2026-07-28) so any harness
    /// can crystallize and recall
    Mcp,
    /// Update to the latest release; --check reports JSON without installing
    Update {
        /// Report installed vs latest as JSON; do not install
        #[arg(long)]
        check: bool,
    },
}

fn main() {
    restore_sigpipe();
    if let Err(err) = run() {
        eprintln!("tulving: {err:#}");
        std::process::exit(1);
    }
}

/// Rust starts with SIGPIPE ignored, so `tulving status | head -1` ends in a
/// panic from `println!` once `head` closes the pipe. Restore the default so a
/// closed pipe ends the process quietly, the way every other CLI tool behaves.
/// The workspace forbids `unsafe`; the `sigpipe` crate owns the one call.
fn restore_sigpipe() {
    sigpipe::reset();
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Every {
            cadence,
            why,
            max_runs,
            for_,
            on_change,
            key,
            until,
            on,
            notify,
            tag,
            cmd,
        } => {
            let ledger = Ledger::open_default()?;
            let cadence_text = cadence.join(" ");
            if let Some(warning) = unattended_warning(&cmd) {
                eprintln!("warning: {warning}");
            }
            let spec = ScheduleSpec {
                argv: cmd,
                cadence: cadence_text,
                why,
                cwd: std::env::current_dir()
                    .ok()
                    .map(|p| p.display().to_string()),
                env: None, // core captures the PATH fingerprint
                origin: Some(serde_json::json!({ "harness": "shell" })),
                max_runs,
                expires_at: None,
                for_,
                until,
                on,
                notify: notify.map(|n| n.split_whitespace().map(str::to_string).collect()),
                on_change,
                key,
                tags: tag,
            };
            let s = ops::crystallize(&ledger, spec)?;
            print_card(&s);
        }
        Command::Add { input, dry_run } => {
            if input != "-" {
                bail!("usage: tulving add -   (JSON spec on stdin)");
            }
            let spec: ScheduleSpec = serde_json::from_reader(std::io::stdin())
                .context("stdin is not a valid schedule spec")?;
            let s = if dry_run {
                ops::plan(spec)?
            } else {
                ops::crystallize(&Ledger::open_default()?, spec)?
            };
            println!("{}", serde_json::to_string_pretty(&s)?);
        }
        Command::List { all } => {
            let ledger = Ledger::open_default()?;
            let schedules = ledger.list_schedules(all)?;
            if schedules.is_empty() {
                println!("No schedules yet.");
                println!(
                    "Keep something running:  tulving every morning --why \"...\" -- <command>"
                );
                return Ok(());
            }
            for s in &schedules {
                let next = s
                    .next_run
                    .map(|t| {
                        t.with_timezone(&chrono::Local)
                            .format("%a %H:%M")
                            .to_string()
                    })
                    .unwrap_or_else(|| "-".into());
                println!(
                    "#{}  {:<28}  {:<16} next: {:<9} runs: {:<4} dies: {}",
                    s.id,
                    display_name(s),
                    s.cadence,
                    next,
                    s.run_count,
                    mortality(s)
                );
            }
        }
        Command::Changed { since } => {
            let ledger = Ledger::open_default()?;
            let since_ts = cadence::parse_since(&since)?;
            let envelopes = ledger.recall(since_ts, true, false, None)?;
            if envelopes.is_empty() {
                println!("Nothing changed since {since}.");
                return Ok(());
            }
            let names = schedule_names(&ledger)?;
            println!("{} change(s) since {since}:", envelopes.len());
            for e in &envelopes {
                println!("  {}", changed_line(e, &names));
            }
        }
        Command::Digest { since } => {
            let ledger = Ledger::open_default()?;
            print_digest(&ledger, &since)?;
        }
        Command::Why { id, text } => {
            let ledger = Ledger::open_default()?;
            if !text.is_empty() {
                ledger.set_why(&id, &text.join(" "))?;
                println!("✓ #{id} now knows why it exists");
                return Ok(());
            }
            let s = ledger
                .get_schedule(&id)?
                .with_context(|| format!("no schedule '{id}'"))?;
            println!(
                "{}",
                s.why.unwrap_or_else(|| {
                    format!("(no reason recorded — set one: tulving why {id} \"...\")")
                })
            );
            println!(
                "created {} · {} runs · status: {}{}",
                s.created_at
                    .with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M"),
                s.run_count,
                s.status,
                s.retired_reason
                    .map(|r| format!(" ({r})"))
                    .unwrap_or_default()
            );
            println!("command: {}", s.argv.join(" "));
        }
        Command::Now { id } => {
            let ledger = Ledger::open_default()?;
            let s = ledger
                .get_schedule(&id)?
                .with_context(|| format!("no schedule '{id}'"))?;
            let e = ops::run_schedule(&ledger, &s, false)?;
            println!("{}", serde_json::to_string_pretty(&e)?);
        }
        Command::Stop { id, all } => {
            let ledger = Ledger::open_default()?;
            match (id, all) {
                (Some(id), false) => {
                    ledger.retire(&id, "stopped by user")?;
                    println!("✓ stopped #{id} — its history stays in the ledger");
                }
                (None, true) => {
                    let active = ledger.list_schedules(false)?;
                    for s in &active {
                        ledger.retire(&s.id, "stopped by user (stop --all)")?;
                    }
                    println!(
                        "✓ stopped {} schedule(s) — history stays in the ledger",
                        active.len()
                    );
                    println!("  (the clock still ticks harmlessly; `tulving uninit` removes it)");
                }
                _ => bail!("give one schedule id, or --all"),
            }
        }
        Command::Snooze { args, all } => {
            let ledger = Ledger::open_default()?;
            // id and duration accepted in either order.
            let mut duration = None;
            let mut id = None;
            for arg in args {
                if duration.is_none() && cadence::parse_duration(&arg).is_ok() {
                    duration = Some(arg);
                } else if id.is_none() {
                    id = Some(arg);
                } else {
                    bail!("too many arguments; expected a schedule id and a duration");
                }
            }
            let Some(duration) = duration else {
                bail!("missing duration (2d, 1w, 6h)");
            };
            let until = chrono::Utc::now() + cadence::parse_duration(&duration)?;
            let local = until
                .with_timezone(&chrono::Local)
                .format("%a %Y-%m-%d %H:%M");
            match (id, all) {
                (Some(id), false) => {
                    ledger.snooze_until(&id, until)?;
                    println!("✓ #{id} snoozed until {local}");
                }
                (None, true) => {
                    let active = ledger.list_schedules(false)?;
                    for s in &active {
                        ledger.snooze_until(&s.id, until)?;
                    }
                    println!("✓ snoozed {} schedule(s) until {local}", active.len());
                }
                _ => bail!("give one schedule id, or --all"),
            }
        }
        Command::Status => {
            let ledger = Ledger::open_default()?;
            print_status(&ledger)?;
        }
        Command::Recall {
            since,
            changed,
            failed,
            schedule,
        } => {
            let ledger = Ledger::open_default()?;
            let since_ts = cadence::parse_since(&since)?;
            let envelopes = ledger.recall(since_ts, changed, failed, schedule.as_deref())?;
            for e in envelopes {
                println!("{}", serde_json::to_string(&e)?);
            }
        }
        Command::Tick => {
            let ledger = Ledger::open_default()?;
            let report = ops::tick(&ledger)?;
            if !report.ran.is_empty() || report.missed_marked > 0 {
                eprintln!(
                    "tick: ran {}, marked {} missed",
                    report.ran.len(),
                    report.missed_marked
                );
            }
        }
        Command::Init => platform::install_timer()?,
        Command::Uninit => platform::remove_timer()?,
        Command::Export { path } => {
            let ledger = Ledger::open_default()?;
            ledger
                .conn
                .execute("VACUUM INTO ?1", tulving_core::rusqlite::params![path])?;
            println!("exported ledger to {path}");
        }
        Command::Mcp => tulving_mcp::serve()?,
        Command::Update { check } => update::run(check)?,
    }
    Ok(())
}

/// Map schedule ids to the most human name available: why, else command.
fn schedule_names(ledger: &Ledger) -> Result<HashMap<String, String>> {
    Ok(ledger
        .list_schedules(true)?
        .into_iter()
        .map(|s| {
            let name = s.why.clone().unwrap_or_else(|| display_name(&s));
            (s.id, name)
        })
        .collect())
}

fn changed_line(e: &Envelope, names: &HashMap<String, String>) -> String {
    let name = names
        .get(&e.schedule_id)
        .cloned()
        .unwrap_or_else(|| format!("#{}", e.schedule_id));
    let when = e.ts.with_timezone(&chrono::Local).format("%a %H:%M");
    let delta = e
        .delta
        .as_ref()
        .map(|d| {
            format!(
                "{} → {}",
                compact(d.get("prev").unwrap_or(&serde_json::Value::Null)),
                compact(d.get("new").unwrap_or(&serde_json::Value::Null))
            )
        })
        .unwrap_or_default();
    format!("{when}  {name}: {delta}")
}

/// One-line JSON, elided so a delta stays readable in a terminal row.
fn compact(v: &serde_json::Value) -> String {
    let s = v.to_string();
    if s.chars().count() <= 40 {
        s
    } else {
        let head: String = s.chars().take(39).collect();
        format!("{head}…")
    }
}

fn print_digest(ledger: &Ledger, since: &str) -> Result<()> {
    let since_ts = cadence::parse_since(since)?;
    let envelopes = ledger.recall(since_ts, false, false, None)?;
    let names = schedule_names(ledger)?;
    let ran = envelopes.iter().filter(|e| !e.missed).count();
    let changed: Vec<&Envelope> = envelopes.iter().filter(|e| e.changed).collect();
    let failed: Vec<&Envelope> = envelopes
        .iter()
        .filter(|e| !e.missed && e.exit != Some(0))
        .collect();
    let missed = envelopes.iter().filter(|e| e.missed).count();

    println!(
        "Since {since}: {ran} run(s), {} changed, {} failed, {missed} missed.",
        changed.len(),
        failed.len()
    );
    if changed.is_empty() && failed.is_empty() && missed == 0 {
        println!("All quiet. The ledger absorbed the routine.");
    }
    if !changed.is_empty() {
        println!("\nChanged:");
        for e in &changed {
            println!("  {}", changed_line(e, &names));
        }
    }
    if !failed.is_empty() {
        println!("\nFailed:");
        for e in &failed {
            let name = names
                .get(&e.schedule_id)
                .cloned()
                .unwrap_or_else(|| format!("#{}", e.schedule_id));
            println!(
                "  {}  {}: exit {:?}",
                e.ts.with_timezone(&chrono::Local).format("%a %H:%M"),
                name,
                e.exit
            );
        }
    }
    let retired: Vec<Schedule> = ledger
        .list_schedules(true)?
        .into_iter()
        .filter(|s| s.status == "retired")
        .collect();
    let recently_ended: Vec<&Schedule> = retired
        .iter()
        .filter(|s| envelopes.iter().any(|e| e.schedule_id == s.id))
        .collect();
    if !recently_ended.is_empty() {
        println!("\nRetired:");
        for s in recently_ended {
            println!(
                "  #{} {} — {}",
                s.id,
                s.why.clone().unwrap_or_else(|| display_name(s)),
                s.retired_reason.clone().unwrap_or_else(|| "done".into())
            );
        }
    }
    Ok(())
}

fn print_status(ledger: &Ledger) -> Result<()> {
    match platform::timer_status() {
        Some(timer) if timer.active => {
            println!("✓ clock    {}", timer.label);
            if let Some(note) = timer.note {
                println!("           {note}");
            }
        }
        Some(timer) => {
            println!("✗ clock    {}", timer.label);
            if let Some(remedy) = timer.remedy {
                println!("           scheduled runs will not fire — run: {remedy}");
            }
            if let Some(note) = timer.note {
                println!("           {note}");
            }
        }
        None => println!("✗ clock    no timer installed — run: tulving init"),
    }
    let path = tulving_core::paths::ledger_path();
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    println!("✓ ledger   {} ({} KB)", path.display(), size / 1024);

    let schedules = ledger.list_schedules(false)?;
    let next = schedules
        .iter()
        .filter_map(|s| s.next_run)
        .min()
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%a %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "-".into());
    println!("✓ watches  {} active, next due {next}", schedules.len());

    let day = chrono::Utc::now() - chrono::Duration::hours(24);
    let recent = ledger.recall(day, false, false, None)?;
    let missed = recent.iter().filter(|e| e.missed).count();
    let failed = recent
        .iter()
        .filter(|e| !e.missed && e.exit != Some(0))
        .count();
    let mark = if missed + failed == 0 { "✓" } else { "!" };
    println!(
        "{mark} last 24h {} run(s), {failed} failed, {missed} missed",
        recent.len() - missed
    );
    Ok(())
}

fn display_name(s: &Schedule) -> String {
    let joined = s.argv.join(" ");
    if joined.chars().count() <= 28 {
        joined
    } else {
        let head: String = joined.chars().take(27).collect();
        format!("{head}…")
    }
}

fn mortality(s: &Schedule) -> String {
    match (&s.max_runs, &s.expires_at, &s.until) {
        (Some(n), _, _) => format!("after {n} runs"),
        (None, Some(t), _) => t
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d")
            .to_string(),
        (None, None, Some(p)) => format!("when {p}"),
        (None, None, None) => "never — consider --for or --max-runs".into(),
    }
}

fn print_card(s: &Schedule) {
    println!("✓ scheduled  #{}", s.id);
    println!("  runs   {}   (cron {})", s.cadence, s.cron);
    println!(
        "  next   {}",
        s.next_run
            .map(|t| t
                .with_timezone(&chrono::Local)
                .format("%a %Y-%m-%d %H:%M")
                .to_string())
            .unwrap_or_else(|| "-".into())
    );
    println!(
        "  why    {}",
        s.why
            .clone()
            .unwrap_or_else(|| format!("(none — set one: tulving why {} \"...\")", s.id))
    );
    println!("  dies   {}", mortality(s));
    println!("  cmd    {}", s.argv.join(" "));
}

/// A scheduled command runs without a terminal. Name the flag a known
/// interactive tool needs before the first run fails with an empty envelope.
fn unattended_warning(argv: &[String]) -> Option<String> {
    let words: Vec<&str> = argv.iter().map(String::as_str).collect();
    let is_rote_play_run = matches!(
        words.as_slice(),
        [program, "play", "run", ..] if program.rsplit('/').next() == Some("rote")
    );
    if is_rote_play_run && !words.iter().any(|w| *w == "--yes" || *w == "-y") {
        return Some(
            "`rote play run` asks for confirmation and this schedule runs without a terminal; add --yes or the run exits 1 with no output"
                .to_string(),
        );
    }
    None
}

#[cfg(test)]
mod unattended_tests {
    use super::unattended_warning;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| (*p).to_string()).collect()
    }

    #[test]
    fn rote_play_run_without_yes_warns() {
        assert!(unattended_warning(&argv(&[
            "rote",
            "play",
            "run",
            "modiqo/x@1.0.0",
            "count=10"
        ]))
        .is_some());
        assert!(
            unattended_warning(&argv(&["/opt/homebrew/bin/rote", "play", "run", "x"])).is_some()
        );
    }

    #[test]
    fn yes_flag_or_other_commands_do_not_warn() {
        assert!(unattended_warning(&argv(&["rote", "play", "run", "x", "--yes"])).is_none());
        assert!(unattended_warning(&argv(&["rote", "play", "run", "x", "-y"])).is_none());
        assert!(unattended_warning(&argv(&["curl", "https://example.com"])).is_none());
        assert!(unattended_warning(&argv(&["rote", "play", "inspect", "x"])).is_none());
    }
}
