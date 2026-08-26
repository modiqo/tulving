# Play × tulving

[Play](https://getrote.dev) ships a host-neutral scheduler contract and,
by design, no scheduler: "The host scheduler owns recurrence, destination
delivery, and storage. Play never installs or fabricates a scheduler."
tulving is that host. Its own probe agrees — `play-scheduler-probe`
reports Claude's session-scoped cron (open-session required, 7-day
expiry) and nothing for the other harnesses.

`play-digest-tick` runs Play's two-phase digest delivery
(`play.digest-delivery/v1`) as one scheduled command:

```bash
tulving every morning --why "Play what's-new digest" \
    --on '.has_updates' -- play-digest-tick
```

Each run prepares the envelope with the host-persisted checkpoint,
delivers the digest into the tulving ledger as the run's result,
acknowledges, releases, and persists the next checkpoint under
`~/.tulving/play-delivery/`. The `--on '.has_updates'` predicate fires
the notifier only when the Play catalog moved; `tulving digest` and
`recall --changed` surface the same signal on the read side. A failed
run never advances the checkpoint — Play's release step refuses without
a delivered acknowledgment — so no window is silently skipped.

## Install

```bash
cp play-digest-tick ~/.local/bin/ && chmod +x ~/.local/bin/play-digest-tick
```

Requires python3, `tulving`, and Play (the script finds `play-delivery`
on PATH, then in `~/tulving/play/scripts/bin/`; override with
`$PLAY_DELIVERY`).

## Scheduling plays themselves

For running a specific Play on a schedule, use the rote shim
(`../rote/rote-schedule`) — a Play executes through `rote play run`.
Plays needing write approval must be blessed first; an unattended run
cannot answer prompts.
