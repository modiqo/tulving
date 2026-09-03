//! The scheduler's verbs: crystallize a spec, run one schedule, and
//! tick everything that is due. Mortality and predicates live here.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, Utc};
use croner::Cron;
use std::io::Write as _;

use crate::db::Ledger;
use crate::model::{short_id, Envelope, Schedule, ScheduleSpec};
use crate::{cadence, config, predicate};

/// Grace period after which an overdue tick is recorded as missed
/// (the run still executes as a catch-up).
const MISSED_GRACE_MIN: i64 = 15;

/// Cadence words are local-time intentions ("morning" means 07:30 where
/// the user lives), so the search runs in local time; storage stays UTC.
pub fn next_occurrence(cron_expr: &str, after: DateTime<Utc>) -> Result<DateTime<Utc>> {
    let cron: Cron = Cron::new(cron_expr)
        .parse()
        .with_context(|| format!("invalid cron '{cron_expr}'"))?;
    let local_after = after.with_timezone(&chrono::Local);
    let next = cron
        .find_next_occurrence(&local_after, false)
        .with_context(|| format!("no next occurrence for '{cron_expr}'"))?;
    Ok(next.with_timezone(&Utc))
}

/// Validate a spec and build the schedule it would create, without
/// writing anything. Producers call this to surface errors — a bad
/// cadence, a predicate typo, an empty command — before committing.
pub fn plan(spec: ScheduleSpec) -> Result<Schedule> {
    if spec.argv.is_empty() {
        bail!("schedule needs a command (argv)");
    }
    // Reject bad predicates at crystallization, not at 7:30 tomorrow.
    for pred in [&spec.until, &spec.on].into_iter().flatten() {
        predicate::validate(pred)?;
    }
    let cron = cadence::to_cron(&spec.cadence)?;
    let now = Utc::now();
    // "for" is the friendly spelling of expires_at; an explicit instant wins.
    let expires_at = match (spec.expires_at, &spec.for_) {
        (Some(t), _) => Some(t),
        (None, Some(d)) => Some(now + cadence::parse_duration(d)?),
        (None, None) => None,
    };
    let schedule = Schedule {
        id: short_id(""),
        argv: spec.argv,
        cadence: spec.cadence,
        cron: cron.clone(),
        why: spec.why,
        cwd: spec.cwd,
        env: spec.env.or_else(env_fingerprint),
        origin: spec.origin,
        max_runs: spec.max_runs,
        expires_at,
        until: spec.until,
        on: spec.on,
        notify: spec.notify,
        on_change: spec.on_change,
        key: spec.key,
        tags: spec.tags,
        created_at: now,
        status: "active".into(),
        retired_reason: None,
        next_run: Some(next_occurrence(&cron, now)?),
        run_count: 0,
    };
    Ok(schedule)
}

/// Crystallize a spec into a stored schedule and return it.
pub fn crystallize(ledger: &Ledger, spec: ScheduleSpec) -> Result<Schedule> {
    let schedule = plan(spec)?;
    ledger.insert_schedule(&schedule)?;
    Ok(schedule)
}

/// PATH captured so the scheduled run sees what the working shell saw.
/// Deliberately not the whole environment: no secrets at rest.
fn env_fingerprint() -> Option<serde_json::Value> {
    let path = std::env::var("PATH").ok()?;
    Some(serde_json::json!({ "PATH": path }))
}

/// Execute one schedule now; append the envelope; apply predicates,
/// the notifier, and stop conditions.
/// The last part of a stream, bounded so a chatty failure cannot bloat the ledger.
pub fn stderr_tail(stderr: &str) -> String {
    const LIMIT: usize = 4096;
    let trimmed = stderr.trim();
    if trimmed.len() <= LIMIT {
        return trimmed.to_string();
    }
    let mut start = trimmed.len() - LIMIT;
    while !trimmed.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", trimmed.get(start..).unwrap_or(trimmed))
}

/// Run one schedule's command now, diff it against the previous envelope,
/// notify if asked, and store the envelope.
pub fn run_schedule(ledger: &Ledger, schedule: &Schedule, missed: bool) -> Result<Envelope> {
    let Some((program, args)) = schedule.argv.split_first() else {
        bail!("schedule {} has an empty command", schedule.id);
    };
    let started = std::time::Instant::now();
    let ts = Utc::now();

    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    if let Some(cwd) = &schedule.cwd {
        cmd.current_dir(cwd);
    }
    if let Some(path) = schedule
        .env
        .as_ref()
        .and_then(|e| e.get("PATH"))
        .and_then(|v| v.as_str())
    {
        cmd.env("PATH", path);
    }

    let output = cmd.output();
    let duration_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);

    let (exit, stdout) = match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr);
            // A command that prints nothing and fails would leave an empty
            // envelope; keep the tail of stderr so the failure is readable.
            let text = if stdout.trim().is_empty() && !stderr.trim().is_empty() {
                format!("stderr: {}", stderr_tail(&stderr))
            } else {
                stdout
            };
            (out.status.code(), text)
        }
        Err(err) => (None, format!("spawn error: {err}")),
    };

    let parsed: Option<serde_json::Value> = serde_json::from_str(stdout.trim()).ok();
    let (result, raw) = match parsed {
        Some(v) => (Some(v), None),
        None => (None, Some(stdout)),
    };

    let previous = ledger.last_envelope(&schedule.id)?;
    let mut stored_result = result.clone();
    let (changed, delta) = match (
        &schedule.key,
        select_array(result.as_ref(), &schedule.on_change),
    ) {
        (Some(key), Some(items)) => match ledger.get_membership(&schedule.id)? {
            None => {
                // Baseline run: the envelope keeps the full snapshot; every
                // later keyed run stores only its delta (docs/DESIGN.md §7).
                ledger.set_membership(&schedule.id, &items)?;
                (false, None)
            }
            Some(prev_items) => {
                let (moved, delta) = keyed_diff(&prev_items, &items, key);
                ledger.set_membership(&schedule.id, &items)?;
                stored_result = None;
                (moved, delta)
            }
        },
        _ if schedule.on_change.is_some() => {
            diff_against_previous(schedule, previous.as_ref(), result.as_ref(), raw.as_deref())
        }
        _ => (false, None),
    };

    let mut envelope = Envelope {
        v: 1,
        run_id: short_id("r_"),
        schedule_id: schedule.id.clone(),
        ts,
        exit,
        result: stored_result,
        raw,
        duration_ms,
        changed,
        missed,
        delta,
        tags: schedule.tags.clone(),
    };

    let current = predicate_input(envelope.result.as_ref(), envelope.raw.as_deref());
    let prior = previous
        .as_ref()
        .map(|p| predicate_input(p.result.as_ref(), p.raw.as_deref()))
        .unwrap_or(serde_json::Value::Null);

    // jq semantics: a predicate whose evaluation errors produced no
    // output, and no output is false. The run itself still counts.
    if let Some(on) = &schedule.on {
        if matches!(predicate::eval_bool(on, &current, &prior), Ok(true)) {
            let tag = match notify(schedule, &envelope) {
                Ok(true) => "notified",
                Ok(false) => "notify-unconfigured",
                Err(_) => "notify-failed",
            };
            envelope.tags.push(tag.into());
        }
    }

    ledger.append_envelope(&envelope)?;

    // Advance the clock, then apply mortality: count, expiry, predicate.
    let next = next_occurrence(&schedule.cron, Utc::now()).ok();
    ledger.set_next_run(&schedule.id, next, true)?;
    let run_count = schedule.run_count + 1;
    if let Some(max) = schedule.max_runs {
        if run_count >= max {
            ledger.retire(&schedule.id, &format!("reached max runs ({max})"))?;
        }
    }
    if let Some(expiry) = schedule.expires_at {
        if Utc::now() >= expiry {
            ledger.retire(&schedule.id, "expired")?;
        }
    }
    if let Some(until) = &schedule.until {
        if matches!(predicate::eval_bool(until, &current, &prior), Ok(true)) {
            ledger.retire(&schedule.id, &format!("stop condition met: {until}"))?;
        }
    }
    Ok(envelope)
}

/// The array a keyed schedule watches: the `on_change` pointer selects
/// it inside the result document; a bare array result is used directly.
fn select_array(
    result: Option<&serde_json::Value>,
    on_change: &Option<String>,
) -> Option<serde_json::Value> {
    let doc = result?;
    let selected = match on_change.as_deref().filter(|p| !p.is_empty() && *p != "*") {
        Some(pointer) => doc.pointer(pointer)?,
        None => doc,
    };
    selected.is_array().then(|| selected.clone())
}

/// Set-diff two membership arrays keyed by a JSON pointer into each item.
/// Items missing the key are identified by their whole serialized value.
fn keyed_diff(
    prev: &serde_json::Value,
    new: &serde_json::Value,
    key: &str,
) -> (bool, Option<serde_json::Value>) {
    use std::collections::BTreeMap;
    let index = |v: &serde_json::Value| -> BTreeMap<String, serde_json::Value> {
        v.as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        let k = item
                            .pointer(key)
                            .map(|kv| match kv.as_str() {
                                Some(text) => text.to_string(),
                                None => kv.to_string(),
                            })
                            .unwrap_or_else(|| item.to_string());
                        (k, item.clone())
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let before = index(prev);
    let after = index(new);
    let added: Vec<&serde_json::Value> = after
        .iter()
        .filter(|(k, _)| !before.contains_key(*k))
        .map(|(_, item)| item)
        .collect();
    let removed: Vec<&serde_json::Value> = before
        .iter()
        .filter(|(k, _)| !after.contains_key(*k))
        .map(|(_, item)| item)
        .collect();
    let changed: Vec<serde_json::Value> = after
        .iter()
        .filter_map(|(k, item)| {
            let old = before.get(k)?;
            (old != item).then(|| serde_json::json!({ "key": k, "prev": old, "new": item }))
        })
        .collect();
    if added.is_empty() && removed.is_empty() && changed.is_empty() {
        return (false, None);
    }
    let delta = serde_json::json!({
        "added": added,
        "removed": removed,
        "changed": changed,
        "counts": {
            "total": after.len(),
            "added": added.len(),
            "removed": removed.len(),
            "changed": changed.len(),
        },
    });
    (true, Some(delta))
}

/// What predicates see: the JSON result, or `{"raw": text}` for
/// commands that do not emit JSON.
fn predicate_input(result: Option<&serde_json::Value>, raw: Option<&str>) -> serde_json::Value {
    match (result, raw) {
        (Some(v), _) => v.clone(),
        (None, Some(text)) => serde_json::json!({ "raw": text }),
        (None, None) => serde_json::Value::Null,
    }
}

/// Run the schedule's notifier (or the config.toml default) with the
/// envelope JSON on stdin. Returns false when no notifier is configured.
fn notify(schedule: &Schedule, envelope: &Envelope) -> Result<bool> {
    let configured;
    let argv = match &schedule.notify {
        Some(argv) => argv,
        None => {
            configured = config::load()?.notify;
            match &configured {
                Some(argv) => argv,
                None => return Ok(false),
            }
        }
    };
    let Some((program, args)) = argv.split_first() else {
        return Ok(false);
    };
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("cannot spawn notifier '{program}'"))?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(serde_json::to_string(envelope)?.as_bytes())?;
    }
    let status = child.wait()?;
    if !status.success() {
        bail!("notifier exited with {status}");
    }
    Ok(true)
}

/// Compare this run's output to the previous envelope's, over an optional
/// JSON-pointer projection, so noisy fields do not cause false alarms.
fn diff_against_previous(
    schedule: &Schedule,
    previous: Option<&Envelope>,
    result: Option<&serde_json::Value>,
    raw: Option<&str>,
) -> (bool, Option<serde_json::Value>) {
    let Some(prev) = previous else {
        return (false, None); // first run: baseline, not a change
    };
    let pointer = schedule
        .on_change
        .as_deref()
        .filter(|p| !p.is_empty() && *p != "*");

    let project = |v: Option<&serde_json::Value>| -> Option<serde_json::Value> {
        let v = v?;
        match pointer {
            Some(p) => v.pointer(p).cloned(),
            None => Some(v.clone()),
        }
    };

    let (old, new) = match (result, raw) {
        (Some(_), _) => (project(prev.result.as_ref()), project(result)),
        (None, Some(text)) => (
            prev.raw.clone().map(serde_json::Value::String),
            Some(serde_json::Value::String(text.to_string())),
        ),
        (None, None) => (None, None),
    };

    if old == new {
        return (false, None);
    }
    (true, Some(serde_json::json!({ "prev": old, "new": new })))
}

/// What one tick did: the envelopes it produced and the misses it marked.
#[derive(Debug)]
pub struct TickReport {
    /// Envelopes appended by this tick, in due order.
    pub ran: Vec<Envelope>,
    /// Overdue runs recorded as missed markers before their catch-up.
    pub missed_marked: usize,
}

/// Run everything due. Overdue past the grace period is recorded as a
/// missed marker first; the run still executes as a catch-up.
pub fn tick(ledger: &Ledger) -> Result<TickReport> {
    let now = Utc::now();
    let due = ledger.due_schedules(now)?;
    let mut ran = Vec::new();
    let mut missed_marked = 0;
    for schedule in due {
        let overdue = schedule
            .next_run
            .is_some_and(|t| now - t > Duration::minutes(MISSED_GRACE_MIN));
        if overdue {
            let marker = Envelope {
                v: 1,
                run_id: short_id("r_"),
                schedule_id: schedule.id.clone(),
                ts: schedule.next_run.unwrap_or(now),
                exit: None,
                result: None,
                raw: None,
                duration_ms: 0,
                changed: false,
                missed: true,
                delta: None,
                tags: schedule.tags.clone(),
            };
            ledger.append_envelope(&marker)?;
            missed_marked += 1;
        }
        ran.push(run_schedule(ledger, &schedule, false)?);
    }
    Ok(TickReport { ran, missed_marked })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_ledger() -> Ledger {
        Ledger::open(std::path::Path::new(":memory:")).unwrap()
    }

    fn spec(argv: &[&str], cadence: &str) -> ScheduleSpec {
        ScheduleSpec {
            argv: argv.iter().map(|s| (*s).to_string()).collect(),
            cadence: cadence.into(),
            why: Some("test watch".into()),
            cwd: None,
            env: None,
            origin: None,
            max_runs: None,
            expires_at: None,
            for_: None,
            until: None,
            on: None,
            notify: None,
            on_change: None,
            key: None,
            tags: vec![],
        }
    }

    #[test]
    fn crystallize_then_run_appends_envelope() {
        let ledger = memory_ledger();
        let s = crystallize(&ledger, spec(&["echo", "{\"n\": 1}"], "hourly")).unwrap();
        assert!(s.next_run.is_some());
        let e = run_schedule(&ledger, &s, false).unwrap();
        assert_eq!(e.exit, Some(0));
        assert_eq!(e.result, Some(serde_json::json!({"n": 1})));
        let got = ledger
            .recall(Utc::now() - Duration::hours(1), false, false, None)
            .unwrap();
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn on_change_flags_only_movement() {
        let ledger = memory_ledger();
        let mut sp = spec(&["echo", "{\"price\": 10}"], "hourly");
        sp.on_change = Some("/price".into());
        let s = crystallize(&ledger, sp).unwrap();
        let first = run_schedule(&ledger, &s, false).unwrap();
        assert!(!first.changed, "first run is a baseline");
        let second = run_schedule(&ledger, &s, false).unwrap();
        assert!(!second.changed, "same projected value is not a change");
    }

    #[test]
    fn max_runs_retires_the_schedule() {
        let ledger = memory_ledger();
        let mut sp = spec(&["true"], "hourly");
        sp.max_runs = Some(1);
        let s = crystallize(&ledger, sp).unwrap();
        run_schedule(&ledger, &s, false).unwrap();
        let after = ledger.get_schedule(&s.id).unwrap().unwrap();
        assert_eq!(after.status, "retired");
    }

    #[test]
    fn until_predicate_retires_when_true() {
        let ledger = memory_ledger();
        let mut sp = spec(&["echo", "{\"state\": \"MERGED\"}"], "hourly");
        sp.until = Some(".state == \"MERGED\"".into());
        let s = crystallize(&ledger, sp).unwrap();
        run_schedule(&ledger, &s, false).unwrap();
        let after = ledger.get_schedule(&s.id).unwrap().unwrap();
        assert_eq!(after.status, "retired");
        assert!(after.retired_reason.unwrap().contains("stop condition"));
    }

    #[test]
    fn on_predicate_with_prev_marks_notify_state() {
        let ledger = memory_ledger();
        let mut sp = spec(&["echo", "{\"dau\": 100}"], "hourly");
        sp.on = Some(".dau < prev.dau".into());
        let s = crystallize(&ledger, sp).unwrap();
        let first = run_schedule(&ledger, &s, false).unwrap();
        assert!(first.tags.is_empty(), "no prev on the first run");
        // Second run has the same dau, so the drop predicate stays false.
        let second = run_schedule(&ledger, &s, false).unwrap();
        assert!(second.tags.is_empty());
    }

    #[test]
    fn keyed_diff_buckets_added_removed_changed() {
        let prev = serde_json::json!([
            {"name": "a", "v": 1}, {"name": "b", "v": 1}, {"name": "c", "v": 1}
        ]);
        let new = serde_json::json!([
            {"name": "a", "v": 1}, {"name": "b", "v": 2}, {"name": "d", "v": 1}
        ]);
        let (moved, delta) = keyed_diff(&prev, &new, "/name");
        assert!(moved);
        let delta = delta.unwrap();
        assert_eq!(delta["counts"]["added"], 1);
        assert_eq!(delta["counts"]["removed"], 1);
        assert_eq!(delta["counts"]["changed"], 1);
        assert_eq!(delta["added"][0]["name"], "d");
        assert_eq!(delta["removed"][0]["name"], "c");
        assert_eq!(delta["changed"][0]["key"], "b");
    }

    #[test]
    fn keyed_schedule_stores_baseline_then_deltas() {
        let ledger = memory_ledger();
        let mut sp = spec(
            &["sh", "-c", "echo \"[{\\\"name\\\": \\\"$N\\\"}]\""],
            "hourly",
        );
        sp.key = Some("/name".into());
        let s = crystallize(&ledger, sp).unwrap();

        std::env::set_var("N", "alpha");
        let first = run_schedule(&ledger, &s, false).unwrap();
        assert!(!first.changed, "baseline is not a change");
        assert!(first.result.is_some(), "baseline keeps the full snapshot");

        std::env::set_var("N", "beta");
        let second = run_schedule(&ledger, &s, false).unwrap();
        assert!(second.changed);
        assert!(second.result.is_none(), "keyed runs store only the delta");
        let delta = second.delta.unwrap();
        assert_eq!(delta["added"][0]["name"], "beta");
        assert_eq!(delta["removed"][0]["name"], "alpha");

        let membership = ledger.get_membership(&s.id).unwrap().unwrap();
        assert_eq!(membership[0]["name"], "beta");
    }

    #[test]
    fn keyed_quiet_run_is_tiny_and_unchanged() {
        let ledger = memory_ledger();
        let mut sp = spec(&["echo", "[{\"name\": \"a\"}]"], "hourly");
        sp.key = Some("/name".into());
        let s = crystallize(&ledger, sp).unwrap();
        run_schedule(&ledger, &s, false).unwrap();
        let quiet = run_schedule(&ledger, &s, false).unwrap();
        assert!(!quiet.changed);
        assert!(quiet.result.is_none());
        assert!(quiet.delta.is_none());
    }

    #[test]
    fn bad_predicate_is_rejected_at_crystallization() {
        let ledger = memory_ledger();
        let mut sp = spec(&["true"], "hourly");
        sp.until = Some(".state ==".into());
        assert!(crystallize(&ledger, sp).is_err());
    }
}


#[cfg(test)]
mod stderr_tail_tests {
    use super::stderr_tail;

    #[test]
    fn short_stderr_is_kept_whole() {
        assert_eq!("boom", stderr_tail("  boom\n"));
    }

    #[test]
    fn long_stderr_keeps_the_tail() {
        let long = "x".repeat(5000);
        let tail = stderr_tail(&long);
        assert!(tail.starts_with('…'));
        assert_eq!(4096 + '…'.len_utf8(), tail.len());
    }
}
