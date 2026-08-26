# tulving — Design

**tulving** is a tiny cron for the agentic world. One binary runs any
command on a schedule and appends every result to a queryable SQLite
ledger. Any harness — Claude Code, rote, a shell — crystallizes and
recalls those schedules in plain language. It is for people who work with
agents and lose track of what those agents promised to check.

Status: draft, pre-1.0. License: Apache-2.0. Language: Rust.

## 1. The goal is a better user, not a better scheduler

Kathy Sierra's rule — make better photographers, not better cameras — is
this project's design test. Cron already exists. What does not exist is a
tool that makes its user better at *follow-through*. Follow-through means
checking again tomorrow, noticing drift, and retiring watches that
finished.

Concretely, a tulving user should become someone who:

- turns "I should keep an eye on that" into a running watch in one
  sentence, instead of forgetting it by Friday;
- starts each morning knowing what moved overnight, because routine
  results were absorbed and only changes surfaced;
- can answer "why does this run?" for every schedule they own, because
  intent is stored next to mechanism;
- carries zero zombie crons, because schedules die by default.

Every feature below is justified by one of those four sentences. A feature
that makes the scheduler better but not the user is out of scope.

## 2. The name states the theory

Endel Tulving distinguished episodic memory (what happened, addressed by
time) from semantic memory (facts). Psychologists add prospective memory:
remembering to act in the future.

Agent harnesses have semantic memory in
abundance — preferences, embeddings, notes. They have almost no episodic or
prospective memory. A harness run today cannot act next Tuesday, and next
Tuesday's run cannot see what today's found.

tulving supplies both missing kinds. The scheduler is prospective memory:
an intention, held until due. The ledger is episodic memory: every result,
appended forever, addressed by time. The digest is consolidation, and it is
not even a feature — it is one more scheduled command that reads the ledger.

## 3. Three nouns and six verbs are the whole surface

Nouns:

- **schedule** — a command (argv array, env, cwd), a cadence, a `why`
  sentence, an origin, and a stop condition;
- **envelope** — one run's record: timestamp, exit code, stdout parsed as
  JSON when it is JSON, duration, a `changed` flag, tags;
- **ledger** — one SQLite file in WAL mode, `~/.tulving/tulving.db`,
  holding all schedules and all envelopes, append-only for envelopes.

Verbs:

| Verb | Job |
|---|---|
| `tulving every <cadence> [flags] -- <cmd>` | crystallize a schedule from a working command |
| `tulving add -` | same, JSON on stdin; what agents and shims call |
| `tulving recall --since <t> [--changed\|--failed\|--origin]` | read the ledger; JSON out |
| `tulving tick` | run whatever is due; called by the OS timer |
| `tulving ls` / `tulving why <id>` | inspect schedules and their intent |
| `tulving retire <id>` | end a schedule; also fired by stop conditions |

Anything not expressible through these verbs is either an edge concern
(section 9) or a non-goal (section 14).

## 4. Nobody writes a cron expression

The notorious failure of cron is not the syntax; it is transcription. The
command that worked in your shell differs from the crontab copy — PATH,
env, flags — and the difference surfaces silently at 8am. tulving removes
transcription: a schedule is crystallized from the command that just ran,
argv and env fingerprint intact. In a terminal, shell history does the
work: `tulving every morning -- !!`. In a harness, the skill composes the
schedule from the verified run in context and shows one confirmation card.

Cadence is plain words — `every morning`, `every 15m`, `weekdays 7:30` — and
a small parser normalizes them to cron syntax inside the database. Cron
syntax survives as a storage format, the way SQL survives under an ORM.
People never read or write it in either direction.

## 5. Every schedule explains itself

Crontab archaeology — staring at a ten-year-old entry, afraid to delete it —
happens because crontabs store mechanism and discard intent. tulving stores
the `why` sentence ("watching DAU after the Aug-25 pricing change") beside
the command, plus the origin: which harness, which session, when. `tulving
why <id>` answers in one line, six months later. The digest composes
against intent, not raw output: "the pricing watch: flat."

## 6. Schedules die by default

Human crons are immortal; agent intentions mostly are not. Agents will
create far more schedules than any person would type, so without built-in
mortality the ledger drowns in zombies within weeks. Every schedule
therefore carries a stop condition: a run count (`--max-runs 14`), a
duration (`--for 2w`), a date, or a predicate over results (`--until
'.state == "MERGED"'`).

A schedule with no stop condition is legal but the
confirmation card nags. When a stop condition fires, the schedule retires
itself and the next digest says so.

## 7. The ledger absorbs routine; people see movement

Because every envelope is retained, "did it change?" is a comparison
against the previous row — no snapshot infrastructure. `--on-change [jq
path]` diffs the new result against the last one, over a projection so
timestamps and request IDs do not cause false alarms. Non-JSON output
falls back to a content hash, with a unified diff stored as the delta.

Predicates see the previous envelope as `prev`, so thresholds come free:
`--on '.dau < prev.dau * 0.95'`. `--on-change` is sugar for `--on '. !=
prev'`. On the read side, `recall --changed` returns only movement, which
makes the digest quiet by design: "checked 41 times, changed twice."

Notification is one hook, not a catalog: `--on <predicate> -- <command>`
runs any command with the delta on stdin. One default notifier command may
be set once in `config.toml`; `--notify` then means "run it." No
integrations ship in this repository, ever.

## 8. Missed runs are reported, never hidden

No OS timer wakes a sleeping laptop. When `tick` runs after a gap, it
marks overdue runs as *missed* in the ledger and executes catch-ups where
the schedule opts in. The digest states misses plainly. This honesty is
the upgrade path: a user who tires of misses adds a remote clock
(section 13), because the ledger showed exactly what was lost.

## 9. The binary is dumb; the edges are smart

The core executes argv, appends envelopes, and evaluates predicates.
Everything differentiated lives at three edges, none of which the core
knows about:

- **The OS is the clock.** `tulving init` registers a launchd agent
  (macOS), a systemd user timer with `Persistent=true` (Linux), a plain
  crontab line (non-systemd), or, later, a Windows scheduled task. Each
  runs `tulving tick` every minute. There is no daemon: nothing to
  babysit, nothing to health-check, nothing running between ticks.
- **The MCP server and skill are the agent surface.** `tulving mcp`
  exposes crystallize-and-recall to every MCP-speaking harness at once.
  The skill owns the one-gesture confirmation card and the session-start
  `recall --since last-session` habit.
- **Shims are one exec.** rote's `play schedule <ref>` composes a pinned
  `rote play run <ref> -y` command and pipes JSON to `tulving add -`.
  Any vendor can do the same in an afternoon, in any language.

## 10. The envelope is the API

The producer contract fits on one page, is versioned, and never breaks:

```json
{
  "v": 1,
  "run_id": "r_9f2c",
  "schedule_id": "a3f2",
  "ts": "2026-08-26T14:30:00Z",
  "exit": 0,
  "result": { "dau": 1198, "wow_change": -1.3 },
  "raw": null,
  "duration_ms": 4120,
  "changed": false,
  "missed": false,
  "origin": { "harness": "rote", "ref": "posthog-project-dau@1.2" },
  "tags": ["pricing-watch"]
}
```

`result` holds stdout when it parses as JSON; otherwise `raw` holds text
and comparisons use its hash. Reserved optional keys (`cost`, `summary`)
are additive. The SQLite schema carries a `schema_version` pragma and
migrations are additive only, so any product may read the ledger with
`sqlite3` and zero tulving code.

## 11. Integration is one exec

Four tiers, each needing no permission from this project and no code
beyond the tier below:

- **Tier 0 — none.** tulving schedules any argv. Users schedule a
  vendor's CLI before the vendor has heard of tulving.
- **Tier 1 — speak the envelope.** Emit JSON on stdout; predicates, stop
  conditions, and rich recall follow automatically. rote's FlowOutput
  already qualifies unchanged.
- **Tier 2 — crystallize natively.** A `schedule` verb in the vendor's
  CLI pipes JSON to `tulving add -`. Detection is polite: offer
  scheduling if `tulving` is on PATH, degrade silently if not.
- **Tier 3 — the agent surface.** Ships with tulving, not with vendors.
  One MCP server serves every harness; vendors integrate by doing
  nothing.

Namespacing makes coexistence work: `origin` is a first-class column, so
several products share one ledger without collision, and `TULVING_HOME`
lets a product embed an isolated ledger instead.

## 12. Prior art was studied, and each failure is a rule here

- **Huginn** (2013, MIT) shipped this exact loop — scheduled agents,
  event store, a digest agent on its own schedule — and decayed under a
  Rails-plus-MySQL footprint and a prebuilt-integration treadmill. Rules
  inherited: single static binary; no integration catalog.
- **Val Town** makes the loop a 30-line idiom (cron vals, per-val SQLite,
  email), but is closed and hosted. Rule: the loop must be self-hostable
  and permissively licensed.
- **Windmill, Inngest, Hatchet, Dagu, Temporal** all store run results
  durably, and all treat them as observability — replay, debugging,
  failure alerts. None offers reader semantics or digests result
  content. Rule: results are mail, not logs; `recall` is the primary
  verb.
- **Healthchecks** has the only native periodic digest in the field, over
  liveness pings that store no output. It diffused because integration
  is one curl. Rules: digest the content, and keep integration at one
  exec.
- **changedetection.io / webchanges** built whole products on
  watch-diff-notify, scoped to web pages. Rule: `--on-change` over
  arbitrary argv generalizes the category, so ship it in the core.
- **OpenClaw** schedules prompts with pruned run history (about seven
  days) and documents that digests are a user-authored pattern, not a
  primitive. Rules: schedule pinned code, retain forever, make the
  digest trivial to compose from `recall`.

## 13. Remote arrives later, and as a choice

The storage seam is a trait: `append(envelope)`, `recall(filter)`,
`claim_due()`. The default backend is the local SQLite file. A future
backend targets celld, a self-hosted distributed Durable Objects runtime:
alarms become a clock that never sleeps, and a replicated cell holds the
ledger for several machines. celld is never required, never fetched by
default, and never mentioned in the quick start. `tulving remote add`
is the moment a user asks for durability across machines, after the local
tool has already earned its keep.

## 14. Non-goals are load-bearing

Each door below is one a prior-art project grew through before losing its
shape. They stay shut:

- no web UI;
- no DAGs, dependencies, or workflow orchestration;
- no retry orchestration beyond a simple per-schedule retry count;
- no integration catalog, in-tree connectors, or per-vendor plugins;
- no embedded runtime — tulving executes argv, nothing else;
- no daemon;
- no required services: no bucket, no account, no database server.

## 15. Implementation shape

Cargo workspace, one released binary:

| Crate | Contents |
|---|---|
| `tulving-core` | schema, envelope, cadence parser, predicate evaluation, scheduler logic; no I/O opinions |
| `tulving-cli` | clap frontend; platform module for launchd/systemd/cron registration |
| `tulving-mcp` | stdio MCP server over `tulving-core` (milestone 3) |

Dependency budget is part of the identity: rusqlite (bundled SQLite),
serde, clap, croner, jaq, and little else. State lives in
`~/.tulving/` (override: `$TULVING_HOME`): `tulving.db` plus its
transient `-wal`/`-shm` sidecars, an optional `config.toml`, and a
`runs/` spill directory for oversized outputs. Backup is `tulving
export`, implemented as `VACUUM INTO`. The ledger is per-user; schedules
run as the user who crystallized them, with that user's credentials.

## 16. Roadmap

1. **M1 — the ledger earns its keep.** Nouns, `every`/`add`/`recall`/
   `ls`/`why`/`retire`/`run`/`tick`, `--on-change`, stop conditions.
   Manual and tick-driven runs. macOS `init`.
2. **M2 — the clock everywhere.** systemd and crontab `init`, missed-run
   catch-up policy, `--on` predicates with `prev`, `export`.
3. **M3 — the agent surface.** `tulving mcp`, the Claude Code skill, the
   rote shim, one example digest Play. Then a two-week dogfood; the exit
   test is whether the digest is still read on day 14 unprompted.
4. **M4 — the protocol goes public.** ENVELOPE.md frozen at v1,
   schema doc, packs (`tulving import <pack.db>`), OSS release.
5. **M5 — remote.** The celld backend behind the storage trait. Windows
   `init` when demand appears.

## 17. Governance

Apache-2.0, in a neutral repository not owned by any producer, rote
included. The envelope spec and SQLite schema are the public API;
both version additively and never break. A vendor evaluating tulving
must find its three answers in five minutes: permissive license, frozen
spec, and no structural advantage for any producer — rote is the first
producer, not the owner.

The first commit is M1. Build it, schedule three real watches, and let
the ledger argue for itself.
