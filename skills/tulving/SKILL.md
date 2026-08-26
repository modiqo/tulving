---
name: tulving
description: Give recurring work a future and a memory with tulving. Use when the user says "keep doing this", "check this every ...", "watch this until ...", "keep an eye on ...", asks "has anything changed" or "what happened while I was away", or at session start to recall what ran since the last session.
---

# tulving — crystallize schedules, recall the ledger

tulving is a tiny cron with a memory. It runs commands on a schedule and
appends every result to a ledger at `~/.tulving/tulving.db`. Your job in
this skill is two gestures: crystallize recurrence when the user asks
for it, and recall the ledger when a session starts or the user asks
what happened. Check the binary exists (`tulving --version`); if absent,
say so and skip — never fake either gesture.

## Gesture 1: crystallize ("keep doing this")

Trigger: the user expresses recurrence about work that just succeeded —
"check this every morning", "keep an eye on DAU until we know",
"watch this PR until it merges".

Rules, in order:

1. **Take the command verbatim from the run that worked.** The argv that
   just succeeded in this session, pinned versions included. Never
   retype or "clean up" a working command.
2. **Compose the full spec yourself.** Cadence from the user's words.
   The `why` is one sentence naming the user's actual reason. Always
   include a stop condition: `max_runs`, `for`, or an `until` predicate
   inferred from the request ("until it merges" ->
   `.state == "MERGED"`). Set `origin` to your harness name and session.
3. **Show one confirmation card, then act.** Present what runs, when,
   why, and when it dies. One yes creates it; never ask a second
   question when a sensible default exists.
4. **Never show cron syntax to the user.** Speak cadence words.

Create with JSON on stdin (works in every harness with a shell):

```bash
echo '{
  "argv": ["rote", "play", "run", "acme/dau@1.2", "-y"],
  "cadence": "weekdays 7:30",
  "why": "watching DAU after the Aug-25 pricing change",
  "for": "2w",
  "on": ".dau < prev.dau * 0.95",
  "on_change": "/dau",
  "origin": {"harness": "claude-code", "session": "<id>"}
}' | tulving add -
```

Spec fields: `argv` (required), `cadence` (required; plain words like
"every morning", "15m", "weekdays 7:30", "weekly monday 9am"), `why`, `max_runs`, `for`
(duration: 2w/14d/6h), `until` (jq predicate that retires the schedule),
`on` (jq predicate that fires the notifier), `notify` (argv), `on_change`
(JSON pointer scoping change detection), `tags`, `origin`. In `until`
and `on`, `prev` names the previous run's result.

Before creating, run `tulving list` and reuse or amend an existing
schedule instead of duplicating it.

If a tulving MCP server is connected, its `schedule` tool takes the same
fields; prefer it over the shell.

## Gesture 2: recall ("what happened?")

At session start, or when the user asks what changed or what they
missed:

```bash
tulving digest --since yesterday    # human-shaped rollup
tulving recall --since 24h --changed   # JSON lines, for your own reasoning
```

Surface only movement — changes, failures, misses, retirements. When
nothing moved, one line says so; do not enumerate quiet runs.

## Answering the user's other questions

| The user asks | Run |
|---|---|
| "what's running?" | `tulving list` |
| "has anything changed?" | `tulving changed` |
| "why does this run?" | `tulving why <id>` |
| "stop that watch" | `tulving stop <id>` |
| "quiet it while I'm away" | `tulving snooze <id> 1w` |
| "is tulving working?" | `tulving status` |

## Never

- Create a schedule the user did not ask for.
- Omit a stop condition without telling the user the schedule is
  immortal.
- Show or ask for cron syntax.
- Parse the digest back out of prose — `recall` gives you JSON.
