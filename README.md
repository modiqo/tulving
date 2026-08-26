# tulving

A tiny cron with a memory, for people who work with agents. One binary
runs any command on a schedule and keeps every result in a queryable
SQLite ledger. Recall returns those results on a timescale.

```bash
tulving every morning --why "pricing watch" --on-change /dau -- rote play run acme/dau -y
tulving recall --since yesterday --changed
```

The first line crystallizes a schedule from a command that just worked.
The second returns, as JSON lines, only the runs whose result moved.

## Install

Build from source (Rust 1.85+), then register the per-user timer:

```bash
cargo install --path crates/tulving-cli
tulving init   # macOS launchd; Linux systemd user timer, crontab fallback
```

`init` writes a launchd agent that runs `tulving tick` every 60 seconds.
There is no daemon. State lives in `~/.tulving/tulving.db` (override with
`$TULVING_HOME`).

## What it does

- **No cron syntax.** Cadence is plain words: `every morning`,
  `every 15m`, `weekdays 7:30`, `monday 9am`. Raw 5-field cron also
  passes through.
- **Every schedule explains itself.** `--why` stores intent beside the
  command; `tulving why <id>` answers "why does this run?" months later.
- **Schedules die by default.** `--max-runs 14` and `--for 2w` retire a
  schedule automatically; the ledger records the reason.
- **Change detection over any command.** `--on-change [/json/pointer]`
  flags runs whose result differs from the previous run, scoped so noisy
  fields stay quiet. Deltas land in the envelope as `{prev, new}`.
- **Catalog watches diff as sets.** `--key /name` treats an array result
  as a membership set: deltas become `{added, removed, changed}` with
  counts, so a registry, package list, or price sheet answers "what
  appeared, what vanished, what moved" by name. After the first run's
  baseline snapshot, keyed watches store only deltas — a quiet run costs
  ~150 bytes, so minutes-cadence watches never bloat the ledger, and any
  past state reconstructs from baseline plus deltas.
- **jq predicates see the previous run.** `--until '.state == "MERGED"'`
  retires a schedule when its work is done; `--on '.dau < prev.dau *
  0.95'` fires the notifier on a five-percent drop. A predicate with a
  typo is rejected at crystallization, not at 7:30 tomorrow.
- **Notification is one hook, not a catalog.** `--notify "<command>"`
  (or a `notify` default in `config.toml`) runs any command with the
  envelope JSON on stdin when `--on` fires.
- **Missed runs are recorded, never hidden.** A sleeping laptop produces
  a `missed` marker and a catch-up run, both visible in `recall`.
- **Agents speak JSON.** `tulving add -` takes a schedule spec on stdin;
  `tulving recall` emits envelopes as JSON lines. Any harness integrates
  with one exec.

## Commands

Every verb answers a question you actually ask a cron with a memory:

| Your question | Command |
|---|---|
| "keep doing this" | `every <cadence> [--why] [--max-runs N] [--for 2w] [--on-change [ptr]] [--until <jq>] [--on <jq>] [--notify <cmd>] -- <cmd>` |
| "what do I have running?" | `list [--all]` |
| "has anything changed?" | `changed [--since 24h]` |
| "what happened?" | `digest [--since today]` |
| "why does this run?" | `why <id>` — add text to set the reason |
| "run it now" | `now <id>` |
| "stop this" | `stop <id>` — history stays in the ledger |
| "quiet it for a while" | `snooze <id> 2d` |
| "is this thing even on?" | `status` |

The machine surface: `add -` (JSON spec on stdin), `recall` (JSON lines
out), `tick` (the OS timer calls it), `init`/`uninit`, `export <path>`,
and `mcp` (serve agent harnesses over stdio).

## MCP

`tulving mcp` serves the Model Context Protocol over stdio, so any
MCP-speaking harness gets six tools: `schedule`, `recall`, `schedules`,
`why`, `run_now`, `retire`. The server is stateless per the 2026-07-28
revision — no handshake required, `server/discover` answers capability
reads, and every call opens the ledger fresh. Clients on older
revisions (2025-06-18, 2025-11-25) still connect: the server answers
`initialize` and negotiates down. Register it in Claude Code with:

```bash
claude mcp add tulving -- tulving mcp
```

## Harness integrations

- **Any agent harness** (Claude Code, Codex, or anything that reads
  skills): install [skills/tulving](skills/tulving/SKILL.md). It teaches
  the two gestures — crystallize a schedule from the command that just
  worked, and recall the ledger at session start.
- **rote**: [integrations/rote](integrations/rote/) ships
  `rote-schedule`, a one-file shim that puts a pinned play on a
  schedule. Integrations live outside the core on purpose — tulving
  knows nothing about its producers.
- **Play**: [integrations/play](integrations/play/) ships
  `play-digest-tick`, which runs Play's host-neutral two-phase digest
  delivery (`play.digest-delivery/v1`) on a tulving schedule — Play
  ships the contract and no scheduler; tulving is the host it expects.

## Design

The design document is [docs/DESIGN.md](docs/DESIGN.md): why results are
mail rather than logs, why there is no daemon, what stays out of scope,
and the roadmap (MCP server, systemd, the optional remote backend).

## License

Apache-2.0. See [LICENSE](LICENSE).
