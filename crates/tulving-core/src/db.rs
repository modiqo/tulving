//! The ledger: one SQLite file in WAL mode holding schedules and
//! envelopes. Envelopes are append-only; migrations are additive only.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::model::{Envelope, Schedule};
use crate::paths;

const SCHEMA_VERSION: i32 = 2;

/// An open handle on the ledger database.
#[derive(Debug)]
pub struct Ledger {
    /// The underlying connection; public so read-side tools can query
    /// the documented schema directly.
    pub conn: Connection,
}

impl Ledger {
    /// Open (creating if needed) the ledger at `$TULVING_HOME`/`~/.tulving`.
    pub fn open_default() -> Result<Self> {
        let home = paths::home();
        std::fs::create_dir_all(&home)
            .with_context(|| format!("cannot create {}", home.display()))?;
        Self::open(&paths::ledger_path())
    }

    /// Open a ledger at an explicit path (tests use `:memory:`).
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("cannot open ledger {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let ledger = Self { conn };
        ledger.migrate()?;
        Ok(ledger)
    }

    fn migrate(&self) -> Result<()> {
        let version: i32 = self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 1 {
            self.conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS schedules (
                    id             TEXT PRIMARY KEY,
                    argv           TEXT NOT NULL,
                    cadence        TEXT NOT NULL,
                    cron           TEXT NOT NULL,
                    why            TEXT,
                    cwd            TEXT,
                    env            TEXT,
                    origin         TEXT,
                    max_runs       INTEGER,
                    expires_at     TEXT,
                    on_change      TEXT,
                    tags           TEXT NOT NULL DEFAULT '[]',
                    created_at     TEXT NOT NULL,
                    status         TEXT NOT NULL DEFAULT 'active',
                    retired_at     TEXT,
                    retired_reason TEXT,
                    next_run       TEXT,
                    run_count      INTEGER NOT NULL DEFAULT 0
                );
                CREATE TABLE IF NOT EXISTS envelopes (
                    run_id      TEXT PRIMARY KEY,
                    schedule_id TEXT NOT NULL REFERENCES schedules(id),
                    ts          TEXT NOT NULL,
                    exit_code   INTEGER,
                    result      TEXT,
                    raw         TEXT,
                    duration_ms INTEGER NOT NULL DEFAULT 0,
                    changed     INTEGER NOT NULL DEFAULT 0,
                    missed      INTEGER NOT NULL DEFAULT 0,
                    delta       TEXT,
                    tags        TEXT NOT NULL DEFAULT '[]'
                );
                CREATE INDEX IF NOT EXISTS idx_envelopes_ts ON envelopes(ts);
                CREATE INDEX IF NOT EXISTS idx_envelopes_schedule
                    ON envelopes(schedule_id, ts);",
            )?;
        }
        if version < 2 {
            // v2: jq predicates and a per-schedule notifier.
            self.conn.execute_batch(
                "ALTER TABLE schedules ADD COLUMN until_pred TEXT;
                 ALTER TABLE schedules ADD COLUMN on_pred TEXT;
                 ALTER TABLE schedules ADD COLUMN notify TEXT;",
            )?;
        }
        self.conn
            .pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(())
    }

    /// Store a newly crystallized schedule.
    pub fn insert_schedule(&self, s: &Schedule) -> Result<()> {
        self.conn.execute(
            "INSERT INTO schedules (id, argv, cadence, cron, why, cwd, env,
                origin, max_runs, expires_at, on_change, tags, created_at,
                status, next_run, run_count, until_pred, on_pred, notify)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,
                ?16,?17,?18,?19)",
            params![
                s.id,
                serde_json::to_string(&s.argv)?,
                s.cadence,
                s.cron,
                s.why,
                s.cwd,
                s.env.as_ref().map(std::string::ToString::to_string),
                s.origin.as_ref().map(std::string::ToString::to_string),
                s.max_runs,
                s.expires_at.map(|t| t.to_rfc3339()),
                s.on_change,
                serde_json::to_string(&s.tags)?,
                s.created_at.to_rfc3339(),
                s.status,
                s.next_run.map(|t| t.to_rfc3339()),
                s.run_count,
                s.until,
                s.on,
                s.notify.as_ref().map(serde_json::to_string).transpose()?,
            ],
        )?;
        Ok(())
    }

    /// Fetch one schedule by id.
    pub fn get_schedule(&self, id: &str) -> Result<Option<Schedule>> {
        self.conn
            .query_row(
                &format!("SELECT {SCHEDULE_COLS} FROM schedules WHERE id = ?1"),
                params![id],
                row_to_schedule,
            )
            .optional()
            .context("reading schedule")
    }

    /// List schedules, active only unless `include_retired`.
    pub fn list_schedules(&self, include_retired: bool) -> Result<Vec<Schedule>> {
        let sql = if include_retired {
            format!("SELECT {SCHEDULE_COLS} FROM schedules ORDER BY created_at")
        } else {
            format!(
                "SELECT {SCHEDULE_COLS} FROM schedules
                 WHERE status = 'active' ORDER BY created_at"
            )
        };
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], row_to_schedule)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Active schedules whose next run is at or before `now`.
    pub fn due_schedules(&self, now: DateTime<Utc>) -> Result<Vec<Schedule>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {SCHEDULE_COLS} FROM schedules
             WHERE status = 'active' AND next_run IS NOT NULL AND next_run <= ?1
             ORDER BY next_run"
        ))?;
        let rows = stmt.query_map(params![now.to_rfc3339()], row_to_schedule)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Advance a schedule's clock; `bump_count` also counts a run.
    pub fn set_next_run(
        &self,
        id: &str,
        next: Option<DateTime<Utc>>,
        bump_count: bool,
    ) -> Result<()> {
        if bump_count {
            self.conn.execute(
                "UPDATE schedules SET next_run = ?2, run_count = run_count + 1
                 WHERE id = ?1",
                params![id, next.map(|t| t.to_rfc3339())],
            )?;
        } else {
            self.conn.execute(
                "UPDATE schedules SET next_run = ?2 WHERE id = ?1",
                params![id, next.map(|t| t.to_rfc3339())],
            )?;
        }
        Ok(())
    }

    /// End a schedule, recording why. Its envelopes stay in the ledger.
    pub fn retire(&self, id: &str, reason: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE schedules SET status = 'retired', retired_at = ?2,
                retired_reason = ?3, next_run = NULL
             WHERE id = ?1",
            params![id, Utc::now().to_rfc3339(), reason],
        )?;
        Ok(())
    }

    /// Set or replace a schedule's stored intent.
    pub fn set_why(&self, id: &str, why: &str) -> Result<()> {
        let rows = self.conn.execute(
            "UPDATE schedules SET why = ?2 WHERE id = ?1",
            params![id, why],
        )?;
        if rows == 0 {
            anyhow::bail!("no schedule '{id}'");
        }
        Ok(())
    }

    /// Quiet an active schedule until an instant, without retiring it.
    /// The regular cadence resumes from that instant.
    pub fn snooze_until(&self, id: &str, until: DateTime<Utc>) -> Result<()> {
        let rows = self.conn.execute(
            "UPDATE schedules SET next_run = ?2
             WHERE id = ?1 AND status = 'active'",
            params![id, until.to_rfc3339()],
        )?;
        if rows == 0 {
            anyhow::bail!("no active schedule '{id}'");
        }
        Ok(())
    }

    /// Append one run's envelope. This is the only write to `envelopes`.
    pub fn append_envelope(&self, e: &Envelope) -> Result<()> {
        self.conn.execute(
            "INSERT INTO envelopes (run_id, schedule_id, ts, exit_code,
                result, raw, duration_ms, changed, missed, delta, tags)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                e.run_id,
                e.schedule_id,
                e.ts.to_rfc3339(),
                e.exit,
                e.result.as_ref().map(std::string::ToString::to_string),
                e.raw,
                e.duration_ms,
                i32::from(e.changed),
                i32::from(e.missed),
                e.delta.as_ref().map(std::string::ToString::to_string),
                serde_json::to_string(&e.tags)?,
            ],
        )?;
        Ok(())
    }

    /// Latest completed (non-missed) envelope for a schedule.
    pub fn last_envelope(&self, schedule_id: &str) -> Result<Option<Envelope>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {ENVELOPE_COLS} FROM envelopes
                     WHERE schedule_id = ?1 AND missed = 0
                     ORDER BY ts DESC LIMIT 1"
                ),
                params![schedule_id],
                row_to_envelope,
            )
            .optional()
            .context("reading last envelope")
    }

    /// Read envelopes since an instant, optionally narrowed to changed
    /// runs, failed runs, or one schedule.
    pub fn recall(
        &self,
        since: DateTime<Utc>,
        changed_only: bool,
        failed_only: bool,
        schedule_id: Option<&str>,
    ) -> Result<Vec<Envelope>> {
        let mut sql = format!("SELECT {ENVELOPE_COLS} FROM envelopes WHERE ts >= ?1");
        if changed_only {
            sql.push_str(" AND changed = 1");
        }
        if failed_only {
            sql.push_str(" AND (exit_code IS NULL OR exit_code != 0) AND missed = 0");
        }
        if schedule_id.is_some() {
            sql.push_str(" AND schedule_id = ?2");
        }
        sql.push_str(" ORDER BY ts");
        let mut stmt = self.conn.prepare(&sql)?;
        let since_s = since.to_rfc3339();
        let rows = match schedule_id {
            Some(id) => stmt.query_map(params![since_s, id], row_to_envelope)?,
            None => stmt.query_map(params![since_s], row_to_envelope)?,
        };
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
}

const SCHEDULE_COLS: &str = "id, argv, cadence, cron, why, cwd, env, origin,
    max_runs, expires_at, on_change, tags, created_at, status,
    retired_reason, next_run, run_count, until_pred, on_pred, notify";

const ENVELOPE_COLS: &str = "run_id, schedule_id, ts, exit_code, result, raw,
    duration_ms, changed, missed, delta, tags";

fn parse_ts(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn row_to_schedule(row: &Row) -> rusqlite::Result<Schedule> {
    let argv: String = row.get(1)?;
    let env: Option<String> = row.get(6)?;
    let origin: Option<String> = row.get(7)?;
    let expires: Option<String> = row.get(9)?;
    let tags: String = row.get(11)?;
    let created: String = row.get(12)?;
    let next: Option<String> = row.get(15)?;
    let notify: Option<String> = row.get(19)?;
    Ok(Schedule {
        id: row.get(0)?,
        argv: serde_json::from_str(&argv).unwrap_or_default(),
        cadence: row.get(2)?,
        cron: row.get(3)?,
        why: row.get(4)?,
        cwd: row.get(5)?,
        env: env.and_then(|s| serde_json::from_str(&s).ok()),
        origin: origin.and_then(|s| serde_json::from_str(&s).ok()),
        max_runs: row.get(8)?,
        expires_at: expires.as_deref().map(parse_ts),
        on_change: row.get(10)?,
        tags: serde_json::from_str(&tags).unwrap_or_default(),
        created_at: parse_ts(&created),
        status: row.get(13)?,
        retired_reason: row.get(14)?,
        next_run: next.as_deref().map(parse_ts),
        run_count: row.get(16)?,
        until: row.get(17)?,
        on: row.get(18)?,
        notify: notify.and_then(|s| serde_json::from_str(&s).ok()),
    })
}

fn row_to_envelope(row: &Row) -> rusqlite::Result<Envelope> {
    let ts: String = row.get(2)?;
    let result: Option<String> = row.get(4)?;
    let delta: Option<String> = row.get(9)?;
    let tags: String = row.get(10)?;
    Ok(Envelope {
        v: 1,
        run_id: row.get(0)?,
        schedule_id: row.get(1)?,
        ts: parse_ts(&ts),
        exit: row.get(3)?,
        result: result.and_then(|s| serde_json::from_str(&s).ok()),
        raw: row.get(5)?,
        duration_ms: row.get(6)?,
        changed: row.get::<_, i32>(7)? != 0,
        missed: row.get::<_, i32>(8)? != 0,
        delta: delta.and_then(|s| serde_json::from_str(&s).ok()),
        tags: serde_json::from_str(&tags).unwrap_or_default(),
    })
}
