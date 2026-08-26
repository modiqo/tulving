//! The three nouns of the ledger: schedule specs, stored schedules, and
//! run envelopes. See docs/DESIGN.md §3 and §10.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// What producers hand to `tulving add -` (JSON on stdin) or what
/// `tulving every` composes from its flags. This is the crystallization
/// contract: the command as it ran, plus intent and mortality.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleSpec {
    /// The command as an argv array, captured verbatim from a working run.
    pub argv: Vec<String>,
    /// Plain-words cadence ("every morning", "weekdays 7:30", "every 15m").
    pub cadence: String,
    /// Why this schedule exists; stored beside the command.
    #[serde(default)]
    pub why: Option<String>,
    /// Working directory for the scheduled runs.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Env fingerprint captured at crystallization (PATH at minimum).
    #[serde(default)]
    pub env: Option<serde_json::Value>,
    /// Who crystallized this: harness, session, pinned reference.
    #[serde(default)]
    pub origin: Option<serde_json::Value>,
    /// Stop after this many runs. A schedule with no stop condition is
    /// legal; the confirmation card nags.
    #[serde(default)]
    pub max_runs: Option<i64>,
    /// Stop at this instant.
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    /// Stop after a duration ("2w", "14d", "6h") — the friendlier way to
    /// say `expires_at`; ignored when `expires_at` is set.
    #[serde(default, rename = "for")]
    pub for_: Option<String>,
    /// Retire when this jq predicate over the result is true
    /// (e.g. `.state == "MERGED"`). `prev` names the previous result.
    #[serde(default)]
    pub until: Option<String>,
    /// Run the notifier when this jq predicate over the result is true
    /// (e.g. `.dau < prev.dau * 0.95`).
    #[serde(default)]
    pub on: Option<String>,
    /// Notifier command (argv) run with the envelope on stdin when `on`
    /// fires; falls back to the `notify` command in config.toml.
    #[serde(default)]
    pub notify: Option<Vec<String>>,
    /// Diff each result against the previous one; optional JSON pointer
    /// (e.g. "/plans/0/price") scopes the comparison.
    #[serde(default)]
    pub on_change: Option<String>,
    /// Free-form tags copied onto every envelope, for recall filtering.
    #[serde(default)]
    pub tags: Vec<String>,
}

/// A stored schedule: the spec plus lifecycle state the ledger maintains.
#[derive(Debug, Clone, Serialize)]
pub struct Schedule {
    /// Short stable identifier, shown as `#id` everywhere.
    pub id: String,
    /// The command as an argv array.
    pub argv: Vec<String>,
    /// The cadence exactly as the user or harness said it.
    pub cadence: String,
    /// The cadence normalized to a 5-field cron expression.
    pub cron: String,
    /// Why this schedule exists.
    pub why: Option<String>,
    /// Working directory for runs.
    pub cwd: Option<String>,
    /// Env fingerprint captured at crystallization.
    pub env: Option<serde_json::Value>,
    /// Who crystallized this schedule.
    pub origin: Option<serde_json::Value>,
    /// Stop after this many runs.
    pub max_runs: Option<i64>,
    /// Stop at this instant.
    pub expires_at: Option<DateTime<Utc>>,
    /// Retire when this predicate over the result is true.
    pub until: Option<String>,
    /// Notify when this predicate over the result is true.
    pub on: Option<String>,
    /// Notifier command (argv) for `on`.
    pub notify: Option<Vec<String>>,
    /// Diff scope for change detection; see [`ScheduleSpec::on_change`].
    pub on_change: Option<String>,
    /// Tags copied onto every envelope.
    pub tags: Vec<String>,
    /// When the schedule was crystallized.
    pub created_at: DateTime<Utc>,
    /// `active` or `retired`.
    pub status: String,
    /// Why the schedule retired, when it did.
    pub retired_reason: Option<String>,
    /// Next due instant; `None` once retired.
    pub next_run: Option<DateTime<Utc>>,
    /// Completed run count.
    pub run_count: i64,
}

/// One run's record in the ledger. Envelope v1; additive changes only.
#[derive(Debug, Clone, Serialize)]
pub struct Envelope {
    /// Envelope format version; always 1 for now.
    pub v: u32,
    /// Unique run identifier.
    pub run_id: String,
    /// The schedule this run belongs to.
    pub schedule_id: String,
    /// When the run started.
    pub ts: DateTime<Utc>,
    /// Process exit code; `None` when the spawn itself failed.
    pub exit: Option<i32>,
    /// stdout, when it parses as JSON.
    pub result: Option<serde_json::Value>,
    /// stdout as text, when it does not parse as JSON.
    pub raw: Option<String>,
    /// Wall-clock duration of the run.
    pub duration_ms: i64,
    /// True when change detection saw movement against the previous run.
    pub changed: bool,
    /// True for a marker recording a run that never happened on time.
    pub missed: bool,
    /// `{prev, new}` when `changed` is true.
    pub delta: Option<serde_json::Value>,
    /// Tags inherited from the schedule.
    pub tags: Vec<String>,
}

/// Short unique id without a rand dependency: nanos mixed with the pid,
/// rendered base36.
pub fn short_id(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mixed = nanos ^ ((u128::from(std::process::id())) << 64);
    let mut n = mixed % 36u128.pow(8);
    let mut s = String::with_capacity(prefix.len() + 8);
    s.push_str(prefix);
    let mut digits = ['0'; 8];
    for slot in digits.iter_mut().rev() {
        let d = u32::try_from(n % 36).unwrap_or(0);
        *slot = char::from_digit(d, 36).unwrap_or('0');
        n /= 36;
    }
    s.extend(digits);
    s
}
