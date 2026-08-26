# The tulving playbook: every command

Reference for humans and for harnesses building abstractions (Play,
skills, MCP clients). Structure mirrors the tool: human verbs, then the
machine surface, then lifecycle and operations. The spec and envelope
formats live in [ENVELOPE.md](ENVELOPE.md).

Conventions: `<id>` is a schedule id from `list`. Durations are `30m`,
`6h`, `2d`, `1w`. Cadences are plain words (`morning`, `daily 9am`,
`weekdays 7:30`, `monday 9am`, `weekly`, `15m`) or raw 5-field cron.
Exit code 0 on success; errors print one line to stderr and exit 1.

## Human verbs

### every — keep doing this

```
tulving every <cadence...> [flags] -- <command...>
```

| Flag | Meaning |
|---|---|
| `--why <text>` | Reason, stored beside the command |
| `--max-runs <N>` | Retire after N runs |
| `--for <dur>` | Retire after a duration |
| `--on-change [ptr]` | Flag runs whose result moved; optional JSON pointer scopes it |
| `--key <ptr>` | Set-diff an array result by this pointer per item |
| `--until <jq>` | Retire when the predicate over the result is true |
| `--on <jq>` | Run the notifier when the predicate is true |
| `--notify <cmd>` | Notifier command (whitespace-split); envelope JSON on stdin |
| `--tag <t>` | Tag envelopes for recall filtering; repeatable |

Prints the confirmation card: id, cadence and cron, next run, why,
mortality, command. In `--until`/`--on`, `prev` names the previous
result. Alias: `create`.

### list — what do I have running?

```
tulving list [--all]
```

One row per schedule: id, command, cadence, next run, run count,
mortality. `--all` includes retired. Alias: `ls`.

### changed — has anything changed?

```
tulving changed [--since 24h]
```

Human-readable lines: time, schedule name (its why), delta. Empty
window prints "Nothing changed since ...".

### digest — what happened?

```
tulving digest [--since today]
```

Counts (runs, changed, failed, missed), then only the interesting
sections: Changed, Failed, Retired.

### why — why does this run?

```
tulving why <id>            # show reason, history, status
tulving why <id> <text...>  # set the reason
```

### now — run it now

```
tulving now <id>
```

Runs regardless of cadence; prints the envelope as JSON. Counts toward
`--max-runs`. Alias: `run`.

### stop — end schedules

```
tulving stop <id>
tulving stop --all
```

Retires; envelopes stay in the ledger forever. Alias: `retire`.

### snooze — quiet without stopping

```
tulving snooze <id> <dur>    # either order: snooze 2d <id>
tulving snooze --all <dur>
```

Pushes the next run; the regular cadence resumes afterward.

### status — is this thing even on?

```
tulving status
```

Four lines: clock (which timer, or how to install one), ledger path and
size, active watches with next due, last-24h run/failed/missed counts.

## Machine surface

### add — crystallize from JSON

```
tulving add -              # spec JSON on stdin; prints the schedule
tulving add - --dry-run    # validate and normalize; writes nothing
```

The producer write verb. Spec fields are in ENVELOPE.md. `--dry-run`
surfaces bad cadences, predicate typos, and empty commands before
anything is committed; on success it returns the normalized schedule
(cron, next run) for rendering a confirmation card.

### recall — read the ledger

```
tulving recall [--since 24h] [--changed] [--failed] [--schedule <id>]
```

Envelopes as JSON lines, oldest first. `--since` accepts `yesterday`,
`today`, durations, or RFC 3339. The inbox primitive: a reader keeps
its own last-checked timestamp and recalls since it.

### tick — run whatever is due

```
tulving tick
```

Called by the OS timer every 60 seconds. Runs due schedules, records
misses past a 15-minute grace as `missed` envelopes before the
catch-up, applies mortality. Never call it from an abstraction.

### mcp — serve agent harnesses

```
tulving mcp
```

Stateless MCP over stdio (spec 2026-07-28; older clients negotiate
down). Tools: `schedule`, `recall`, `schedules`, `why`, `run_now`,
`retire`.

## Lifecycle and operations

### init / uninit — the clock

```
tulving init      # launchd (macOS) or systemd user timer / crontab (Linux)
tulving uninit    # remove the timer; schedules untouched
```

No daemon exists; the OS timer runs `tick` every 60 seconds. `uninit`
pauses everything reversibly.

### export — backup

```
tulving export <path>
```

Safe copy via `VACUUM INTO`; use this instead of copying the db file.

### update — pull the latest release

```
tulving update --check   # {"installed": "...", "latest": "...", "update_available": bool}
tulving update           # install it
```

`--check` is JSON so a harness can offer the update. Install detects a
Homebrew-managed binary and delegates to `brew upgrade`; otherwise it
downloads the release for this platform and replaces the binary via a
fresh inode (an in-place overwrite stales the macOS signature cache and
the kernel kills the binary).

## Abstraction guide (for Play and other harnesses)

- Write through `add -`, never `every`; always `--dry-run` first, then
  render your own card from the normalized schedule, then commit on one
  approval. Always set `origin` and propose a stop condition.
- Read through `recall` JSON, never by scraping human output. Keep your
  own last-checked timestamp; acknowledgment is reader state and
  tulving deliberately has none.
- Dedupe before creating: check `list --all` and amend rather than
  duplicate.
- Failures belong in the ledger and in the inbox; filter at read time,
  never suppress at write time.
- Check `status` before the first create; if no clock is installed the
  schedule will never fire — surface `tulving init`.
- Offer `now <id>` right after creating, so the user sees the first
  envelope immediately.
- Retirements are schedule-state, not envelopes: diff `list --all`
  status across checks to report "this watch ended".
