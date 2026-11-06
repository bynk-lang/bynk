# Title: Prefer bynk-lsp over bynkc-lsp; stop downloading bynkc (bynk-lang/bynk#2936)

(File in bynk-lang/vscode-bynk. Body below.)

Companion to bynk-lang/bynk#2936 (crate-hygiene track, bynk-lang/bynk#2775, E4a; implementing PR bynk-lang/bynk#2946). The language server binary is renamed `bynkc-lsp` → `bynk-lsp`. The main repo's release ships **both** names for one release (archives, loose assets `bynk-lsp-<target>` and `bynkc-lsp-<target>`, both in `SHA256SUMS`); the next release drops `bynkc-lsp` (E4b). This extension needs one release that works across the change.

## To do
- **Prefer `bynk-lsp`, fall back to `bynkc-lsp`** when resolving the server (`src/server.ts`: setting → PATH → cached → download), including the PATH lookup and the download-on-activate asset name.
- **`bynk.executablePath` default** becomes `bynk-lsp` (package.json, README, `walkthroughs/welcome.md`).
- **Stop downloading `bynkc`**: `scripts/fetch-toolchain.mjs` (`["bynkc-lsp", "bynk", "bynkc"]`) and `scripts/check-server-pin.sh` fetch/verify `bynk-lsp` and `bynk` only. (`bynkc` is itself being retired in the track, E3.)
- **`src/doctorReport.ts`** keys on `row.label === "bynkc-lsp"`; `bynk doctor --json` now labels the editor row `bynk-lsp`. Match both labels.
- Update comments/docs mentioning `bynkc-lsp` (`src/extension.ts`, `src/web/extension.ts`, tests under `test/`, the `//bynkServerVersion` notes in `package.json`).
- The pin (`bynkServerVersion`) must point at a release that contains `bynk-lsp-<target>` assets (the E4a release or later); the fallback keeps older pins working.

## Question to confirm
Does anything check the server's `serverInfo.name`? A code search found only `src/web/server.ts` setting its own (`bynk-web`) and nothing reading `bynkc-lsp` there. The native server now reports `bynk-lsp` in the `initialize` reply; please confirm no client logic depends on the old string.

## Ordering
Ship this before the E4b release removes the `bynkc-lsp` assets, ideally in the same release as the E3 `bynkc` alias removal so the extension makes one move.
