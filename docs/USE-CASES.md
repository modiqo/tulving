# How to use tulving: ten use cases

A scenario-driven companion to [PLAYBOOK.md](PLAYBOOK.md) (the command
reference) and [ENVELOPE.md](ENVELOPE.md) (the producer contract). Each
use case shows the human moment, the commands, what the ledger does
over time, and what an abstraction such as Play should build around it.

Shared setup, once per machine:

```bash
brew install modiqo/tap/tulving   # or the curl installer
tulving init                      # per-user OS timer; no daemon
tulving status                    # confirm the clock is on
```

## 0. Crystallize anything an agent just did (the core gesture)

**Moment:** a command worked in a session; the user says "keep doing
this every morning."

```bash
echo '{
  "argv": ["rote", "play", "run", "acme/dau@1.2", "-y"],
  "cadence": "every morning",
  "why": "watching DAU after the pricing change",
  "for": "2w",
  "origin": {"harness": "play", "session": "<id>", "ref": "acme/dau@1.2"}
}' | tulving add - --dry-run     # validate; nothing written
# render a card from the normalized result; one approval; then:
... | tulving add -              # same spec, committed
tulving now <id>                 # optional: run once so the first envelope exists
```

**Over time:** the OS timer runs it; every result becomes an envelope.

**Abstraction:** always dry-run first — bad cadences and predicate
typos fail before anything exists. Take argv verbatim from the run that
worked. Always set `origin`; always propose a stop condition. Check
`tulving list` first and amend instead of duplicating.

## 1. Watch a metric after a change

**Moment:** "tell me if DAU drops after Tuesday's pricing change."

```bash
tulving every morning --why "pricing-change watch" --for 2w \
  --on-change /dau \
  --on '.dau < prev.dau * 0.95' --notify "rote play run me/telegram-notify" \
  -- rote play run acme/dau@1.2 -y
```

**Over time:** quiet mornings append quiet envelopes. A drop past five
percent fires the notifier with the envelope on stdin. After two weeks
the watch retires itself.

**Abstraction:** translate thresholds from user words into `--on` with
`prev`; translate "until we know" into `--for` or `--until`.

## 2. Watch a thing until it resolves

**Moment:** "watch this PR until it merges."

```bash
tulving every 15m --why "PR 4123 until merged" \
  --until '.state == "MERGED"' \
  -- gh pr view 4123 --json state
```

**Over time:** the schedule retires itself the first time the predicate
is true, with the reason stored. The digest reports the retirement.

**Abstraction:** infer the `--until` predicate from the outcome the
user named. Temporary intentions should never need manual cleanup.

## 3. Watch a catalog for what's new

**Moment:** "tell me when new plays land in my org."

```bash
tulving every 6h --why "watch modiqo plays" --on-change --key /name \
  -- rote registry play list --org modiqo --json
```

**Over time:** the first run stores the membership baseline; later runs
store only `{added, removed, changed}` deltas (~150 bytes when quiet).
Removals are detected too — timestamp windows cannot do that.

**Abstraction:** any listing becomes a watch this way. Join `added`
against the user's scheduled refs to flag stale pins ("acme/dau
updated; your watch pins @1.2").

## 4. Schedule a play on a recurring review

**Moment:** "run my issues-review play every Monday at 9."

```bash
rote-schedule chetan/list-my-github-issues@0.1.1 "weekly monday 9am" \
  --why "weekly review of my modiqo/rote issues" --for 30d -- repo=modiqo/rote
```

**Over time:** the pinned reference runs unattended with the user's
credentials; results accumulate as a series.

**Abstraction:** pin the version; pass play parameters after `--`.
Plays needing write approval must be blessed in rote first — an
unattended run cannot answer prompts.

## 5. Check the inbox ("what happened while I was away?")

**Moment:** the user asks `/play check inbox`, or a session starts.

```bash
tulving recall --since <last-checked> --changed   # movement
tulving recall --since <last-checked> --failed    # trouble
tulving list --all                                # status diff finds retirements
```

**Over time:** every scheduled result is already in the ledger; reading
is instant and offline.

**Abstraction:** the reader owns its checkpoint. Keep a last-checked
timestamp in Play's state, recall since it, render changed + failed +
newly-retired, then advance the checkpoint only after display. Empty
inbox gets one calm line, not silence. Humans get the same view as
`tulving digest`; agents should consume the recall JSON directly.

## 6. Keep verifying work an agent built

**Moment:** an agent shipped something; nobody wants to discover
breakage three weeks later.

```bash
tulving every daily 6am --why "smoke: checkout flow still passes" \
  --on '.exit != 0' --notify "play notify-me" \
  -- ./scripts/smoke-checkout.sh
```

**Over time:** failures land in the ledger and the inbox; the next
session — any harness — reads the failure and can fix it.

**Abstraction:** offer this at the end of build tasks: "want me to keep
checking this?" Failure envelopes are inbox items, never suppressed.

## 7. Quiet everything during cleanup or vacation

**Moment:** "I'm cleaning up my rote workspaces" or "I'm out for a
week."

```bash
tulving snooze --all 1w    # cadences resume on their own
tulving stop --all         # or: retire everything; history stays
tulving uninit             # or: remove the clock; schedules untouched
```

**Abstraction:** map the user's phrasing to the right level. Snooze is
almost always the answer; stop is for real resets; uninit is the
reversible master switch.

## 8. Trust check ("is this thing even on?")

**Moment:** the classic cron doubt.

```bash
tulving status     # clock, ledger, next due, last-24h health
tulving now <id>   # force one run and read its envelope
```

**Over time:** a sleeping laptop produces `missed` markers and
catch-ups, visible in `recall` and the digest — never silent skips.

**Abstraction:** run `status` before the first create in any session;
if no clock is installed, surface `tulving init` immediately.

## 9. Check for updates (and pull them)

**Moment:** Play wants users on the latest tulving without asking them
to think about versions.

```bash
tulving update --check
# {"installed":"0.1.2","latest":"0.1.2","update_available":false}
tulving update            # installs; brew-managed binaries delegate to brew
```

**Over time — dogfood it:** tulving can watch its own freshness:

```bash
tulving every weekly --why "tulving update check" \
  --on '.update_available' --notify "play notify-me" \
  -- tulving update --check
```

**Abstraction:** run `update --check` opportunistically (Play install,
session start, or the scheduled watch above) and offer the one-word
update when `update_available` is true. Never auto-install without the
user's standing approval; the JSON is machine-shaped so the offer can
be one line.

## The five rules under all ten

1. Write through `add -` with `--dry-run` first; read through `recall`
   JSON. Human renderings (`digest`, `changed`) are for terminals.
2. Every schedule carries a why and, almost always, a death.
3. Failures and misses are recorded and surfaced, never suppressed.
4. The ledger is append-only history; readers own their checkpoints.
5. tulving knows nothing about its producers — the abstraction owns
   translation from human intent to spec, and from envelopes to
   display.
