---
level: patch
changelog: `bynkc test` now heads a `system` suite's results `system · <suite>:` rather than `integration · <suite>:`; suites still run in heading order, so in a project with unit suites too a `system` suite can now sort among them differently, in the human output and in the `--format json` `suites` array (#2185)
---
