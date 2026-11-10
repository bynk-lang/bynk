---
level: patch
probes: diagnostic_coverage, explained_codes
changelog: An expression written as a statement gets `bynk.parse.bare_expression_statement`, with a machine-applicable `let _ <-` (effectful call) or `let _ =` fix that `bynk check --fix` and the editor apply, instead of "expected `}`"
---
