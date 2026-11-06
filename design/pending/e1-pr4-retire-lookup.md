---
level: patch
changelog: Crate hygiene E1-E2 PR 4 - the `bynkc` lookup (`BYNK_BYNKC`, `PATH`, sibling) and ADR 0434's version-skew gate are retired; `BYNK_BYNKC` is ignored with a one-line warning, `--allow-skew` and `BYNK_ALLOW_SKEW` are accepted and ignored, and the `bynk doctor` compiler row is always in-process
---

## ADR: retire-bynkc-lookup-and-skew-gate
title: bynk compiles in-process everywhere; the bynkc lookup and the version-skew gate are retired
summary: Implements ADR 0736 for E1-E2: what happens to BYNK_BYNKC, --allow-skew and doctor's compiler row

**Context.** ADR 0736 decided that `bynk` runs every command in-process and that
ADR 0434's skew gate goes. `bynk compile` and `bynk test` (PRs 2 and 3) were the
last commands that started a `bynkc`; `check`, `fmt`, `dev` and `deploy` still
did under a `BYNK_BYNKC` override. `vscode-bynk` sets `BYNK_BYNKC` from its
`bynk.compilerPath` setting on every `bynk` call, so a pinned extension release
still sets it.

**Decision.** The lookup (`BYNK_BYNKC`, then `PATH`, then a sibling), the skew
classifier and gate, `shell::delegate*` and the `&Compiler` parameters are
removed. For one release `BYNK_BYNKC` is read only to print one stderr line
saying it is ignored (nothing else changes: the command runs on its own
compiler), and `--allow-skew` (hidden) and `BYNK_ALLOW_SKEW` are accepted and
ignored. `bynk doctor` keeps the JSON `compiler` object and its keys, now
`resolved: true`, `origin: "in-process"`, `version` the driver's, `path` and
`skew` null, so editor consumers keep parsing it. All of it is removed with the
`bynkc` alias (E3).

**Consequences.** A pin set through `BYNK_BYNKC` stops governing the result;
that is the behaviour change, and the warning is how a user learns of it.
`--accept-compatible-skew` (callee contract skew, ADR 0648) is a different
mechanism and is untouched.
