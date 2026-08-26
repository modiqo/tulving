# rote × tulving

Give a [rote](https://modiqo.ai) play a future: run it on a schedule,
keep every result in the tulving ledger, and recall it on a timescale.

```bash
rote-schedule acme/dau@1.2 "every morning" --why "pricing watch" --for 2w
```

This composes the pinned `rote play run acme/dau@1.2 -y` command and
registers it with `tulving add -`. From then on the OS timer runs the
play, each run's FlowOutput lands in the ledger as an envelope, and
`tulving digest` reports what moved.

## Why this integration is one shell script

rote already satisfies tulving's producer contract (docs/DESIGN.md §11):
`rote play run` is non-interactive with `-y`, version-pinned by
reference, and emits structured output. So Tier 2 needs no code inside
either project — just this shim composing a spec. It lives in
`integrations/` to keep that boundary visible: tulving core knows
nothing about rote.

## Install

```bash
cp rote-schedule ~/.local/bin/ && chmod +x ~/.local/bin/rote-schedule
```

Requires `tulving` and `rote` on PATH. Run `rote-schedule --help` for
all flags; play parameters go after `--`:

```bash
rote-schedule me/pr-state "every 15m" --until '.state == "MERGED"' -- pr=4123
```

## Caveats

- The shim escapes quotes and backslashes in arguments; control
  characters in play parameters are out of scope.
- Scheduled runs use the credentials of the user who crystallized the
  schedule, like every tulving run.
- Plays needing write approval must be blessed in rote first; an
  unattended run cannot answer prompts.
