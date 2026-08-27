# tulving

A tiny cron with a memory, for people who work with agents. One binary
runs any command on a schedule and keeps every result in a queryable
SQLite ledger. Recall returns those results on a timescale.

```bash
brew install modiqo/tap/tulving && tulving init
tulving every morning --why "pricing watch" --on-change /dau -- rote play run acme/dau -y
tulving recall --since yesterday --changed
```

The first line installs the binary and registers a per-user OS timer.
The second turns a command that just worked into a morning watch. The
third returns, as JSON lines, only the runs whose result moved.

## The measure is what you become, not what it does

Kathy Sierra's rule — make better photographers, not better cameras —
is this project's design test. Cron already exists. tulving exists to
make you better at *follow-through*, and every command is justified by
one of four sentences. A tulving user becomes someone who:

- turns "I should keep an eye on that" into a running watch in one
  sentence, instead of forgetting it by Friday;
- starts each morning knowing what moved overnight, because routine
  results were absorbed and only changes surfaced;
- can answer "why does this run?" for every schedule they own, because
  intent is stored next to mechanism;
- carries zero zombie crons, because schedules die by default.

The name honors Endel Tulving, who distinguished episodic memory (what
happened, addressed by time) from semantic memory (facts). Agents have
semantic memory in abundance. tulving supplies the missing kinds: the
scheduler is prospective memory — an intention held until due — and the
ledger is episodic memory.

## Your first watch, in sixty seconds

Take any command that just worked and put `tulving every <cadence> --`
in front of it:

```bash
tulving every 6h --why "watch modiqo plays" --on-change --key /name \
  -- rote registry play list --org modiqo --json
```

You see a confirmation card: what runs, when, why, and when it dies.
From then on the OS timer runs it, every result lands in the ledger,
and three questions have instant answers:

```bash
tulving changed          # has anything changed?
tulving digest           # what happened today?
tulving why <id>         # why does this run?
```

No cron syntax appears anywhere. Cadence is plain words — `every
morning`, `15m`, `weekdays 7:30`, `monday 9am`, `weekly` — and raw 5-field cron
still passes through for those who want it.

## Commands are the questions you already ask

| Your question | Command |
|---|---|
| "keep doing this" | `every <cadence> [--why] [--max-runs N] [--for 2w] [--on-change [ptr]] [--key /ptr] [--until <jq>] [--on <jq>] [--notify <cmd>] -- <cmd>` |
| "what do I have running?" | `list [--all]` |
| "has anything changed?" | `changed [--since 24h]` |
| "what happened?" | `digest [--since today]` |
| "why does this run?" | `why <id>` — add text to set the reason |
| "run it now" | `now <id>` |
| "stop this" | `stop <id>` — or `stop --all`; history stays in the ledger |
| "quiet it for a while" | `snooze <id> 2d` — or `snooze --all 1w` |
| "is this thing even on?" | `status` |

The machine surface: `add -` (JSON spec on stdin), `recall` (JSON lines
out), `tick` (the OS timer calls it), `init`/`uninit`, `export <path>`,
`mcp` (serve agent harnesses over stdio), and `update [--check]`.
The complete reference is [docs/PLAYBOOK.md](docs/PLAYBOOK.md); the
use-case guide for harness builders is [docs/USE-CASES.md](docs/USE-CASES.md).

## The habits it teaches

Good tools train judgment as a side effect of use. tulving pushes four
habits every time you touch it:

- **State your reason.** The confirmation card nags when `--why` is
  empty, because six months from now "why does this run?" is the only
  question that matters. Repair intent any time: `tulving why <id>
  "..."`.
- **Give everything an ending.** A schedule with no stop condition is
  legal, and the card says "dies: never" loudly enough to sting. Stop
  conditions are one flag: `--max-runs 14`, `--for 2w`, or `--until
  '.state == "MERGED"'` — and retirement lands in the digest.
- **Watch for movement, not noise.** `--on-change /dau` diffs against
  the previous run over a projection, so timestamps and request IDs
  never cry wolf. `--key /name` treats an array result as a membership
  set: deltas become `{added, removed, changed}` with counts, and after
  the first run's baseline the ledger stores only deltas — a quiet run
  costs about 150 bytes.
- **Trust, then verify in one line.** `status` answers the classic cron
  doubt — is this thing even on? — with the timer, the ledger, the next
  due run, and the last 24 hours. Missed runs are recorded and
  reported, never hidden: no OS timer wakes a sleeping laptop, so
  honesty beats pretense.

jq predicates see the previous run as `prev`, so `--on '.dau < prev.dau
* 0.95'` fires the notifier on a five-percent drop. A predicate with a
typo is rejected at crystallization, not at 7:30 tomorrow. Notification
is one hook, not a catalog: the notifier is any command, receiving the
envelope JSON on stdin.

## Agents get the same two gestures

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

The bundled skill ([skills/tulving](skills/tulving/SKILL.md)) works in
any harness that reads skills — Claude Code, Codex, and others. It
teaches the two gestures: crystallize a schedule from the command that
just worked, and recall the ledger at session start. Agents gain a
future tense — "I'll verify this tomorrow" becomes a schedule that
actually runs — and every session opens knowing what happened since the
last one.

## Integrations live outside the core

tulving knows nothing about its producers; anything that can exec a
command integrates in one line. Two ship as examples:

- **rote**: [integrations/rote](integrations/rote/) ships
  `rote-schedule`, a one-file shim that puts a pinned play on a
  schedule, plus the registry what's-new watch recipe.
- **Play**: [integrations/play](integrations/play/) ships
  `play-digest-tick`, which runs Play's host-neutral two-phase digest
  delivery (`play.digest-delivery/v1`) on a tulving schedule — Play
  ships the contract and no scheduler; tulving is the host it expects.

The producer contract is one page: [docs/ENVELOPE.md](docs/ENVELOPE.md).
Emit JSON on stdout and predicates, stop conditions, and rich recall
follow automatically.

## Install

Homebrew (macOS and Linux):

```bash
brew install modiqo/tap/tulving
```

Or the one-line installer:

```bash
curl -fsSL https://raw.githubusercontent.com/modiqo/tulving/main/install.sh | sh
```

Or from source (Rust 1.85+): `cargo install --path crates/tulving-cli`.

Then register the timer: `tulving init` — launchd on macOS, a systemd
user timer on Linux (crontab fallback). There is no daemon. State lives
in `~/.tulving/tulving.db` (override with `$TULVING_HOME`); back it up
with `tulving export`.

## What it deliberately is not

The boundaries are load-bearing; each is a door a prior tool grew
through before losing its shape. No web UI. No DAGs or workflow
orchestration. No integration catalog. No embedded runtime — tulving
executes argv, nothing else. No daemon. No required services: no
bucket, no account, no database server.

## Design

The design document is [docs/DESIGN.md](docs/DESIGN.md): why results
are mail rather than logs, why there is no daemon, the prior art and
what each failure taught, and the roadmap.

## License

Dual-licensed under [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT),
at your option.
