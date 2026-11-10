# 0736 — bynk and bynk-lsp are the only release binaries; bynkc-lsp is retired

- **Status:** Accepted (v0.508.12)

**Context.** The release shipped `bynkc`, `bynk` and `bynkc-lsp`, with a
`bynk`↔`bynkc` version-skew gate (ADR 0434) from the days when `bynk` shelled out
to `bynkc`. ADR 0101 linked the compiler in-process, so the second binary and its
skew check no longer protect anything. Meanwhile the `bynk-lsp` crate built a
binary named `bynkc-lsp`, so `cargo install bynk-lsp` installed a different name.
The crate-hygiene track (#2775, `design/archive/retired-tracks.md` §9 E4, §11 Q-E2,
§13) settles the end state.

**Decision.** `bynk`, plus optionally `bynk-lsp`, are the only release binaries,
and `bynk` runs every command in-process. The language server is `bynk-lsp`
everywhere: the `[[bin]]` name, `serverInfo.name`, log prefixes, release assets
(`bynk-lsp-<target>`), installers, the devcontainer feature, `bynk doctor` and the
docs. `bynkc-lsp` is retired before 1.0.0 in two steps: the first release ships a
`bynkc-lsp` copy beside `bynk-lsp` (archives, loose assets, installers, feature;
`bynk doctor` accepts it) so editor configs we cannot update keep working, and the
next release removes it (E4b). This amends ADR 0101 (the in-process driver is the
only compiler entry; `bynkc` follows the track's own retirement path, E1–E3) and
retires ADR 0434's skew gate: with one driver binary there is nothing to skew
against. The gate's code is removed by the later E1/E2 slices; this ADR records
the decision.

**Consequences.** `cargo install bynk-lsp` installs `bynk-lsp`. Users with
`cmd = "bynkc-lsp"` have one release to change it to `bynk-lsp`. `vscode-bynk`
(separate repository) prefers `bynk-lsp`, falls back to `bynkc-lsp` and stops
downloading `bynkc`. ADRs that name `bynkc-lsp` are left as written; they record
what was true when decided.
