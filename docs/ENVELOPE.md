# The envelope: tulving's producer contract, v1

This page is the whole integration API. A producer is any command
tulving runs on a schedule. The contract versions additively and never
breaks; `v` marks the envelope format.

## What a producer emits

Write JSON to stdout and exit 0 on success. That is the entire
requirement. Text output is also legal: it is stored as `raw` and
change detection falls back to a content hash.

Reserved optional keys a producer may include in its JSON: `cost`,
`summary`. Both are additive; unknown keys pass through untouched.

## What the ledger stores per run

```json
{
  "v": 1,
  "run_id": "r_9f2c",
  "schedule_id": "a3f2",
  "ts": "2026-08-26T14:30:00Z",
  "exit": 0,
  "result": { "dau": 1198 },
  "raw": null,
  "duration_ms": 4120,
  "changed": false,
  "missed": false,
  "delta": null,
  "tags": ["pricing-watch"]
}
```

- `result` holds stdout when it parses as JSON; otherwise `raw` holds
  the text.
- `exit` is `null` when the spawn itself failed.
- `missed: true` marks a run that never happened on time; the catch-up
  run follows as its own envelope.
- `delta` is `{prev, new}` for plain change detection, or
  `{added, removed, changed, counts}` for keyed set-diffs. Keyed
  schedules store the full snapshot only on their first run; later
  envelopes carry `result: null` and the delta.

## What a schedule spec accepts

`tulving add -` reads this JSON on stdin; the MCP `schedule` tool takes
the same fields:

| Field | Meaning |
|---|---|
| `argv` (required) | The command as an argv array, verbatim |
| `cadence` (required) | Plain words: "every morning", "15m", "weekdays 7:30" |
| `why` | Why this schedule exists |
| `max_runs` | Retire after N runs |
| `for` | Retire after a duration: "2w", "14d", "6h" |
| `expires_at` | Retire at an RFC 3339 instant (wins over `for`) |
| `until` | jq predicate over the result; true retires the schedule |
| `on` | jq predicate; true runs the notifier |
| `notify` | Notifier argv; receives the envelope JSON on stdin |
| `on_change` | Diff against the previous run; optional JSON pointer scopes it |
| `key` | JSON pointer identifying items in an array result; enables set-diff |
| `tags` | Copied onto every envelope |
| `origin` | Who crystallized this: harness, session, pinned reference |

In `until` and `on`, `prev` names the previous run's result.

## Reading the ledger

`tulving recall --since <t>` emits envelopes as JSON lines. The SQLite
schema is documented and versioned (`PRAGMA user_version`); migrations
are additive only, so reading `~/.tulving/tulving.db` directly with any
SQLite client is supported.
