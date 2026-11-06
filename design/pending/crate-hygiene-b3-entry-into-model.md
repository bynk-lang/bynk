---
level: patch
probes: model_readers_beyond_emit
changelog: The entry handlers' signatures become system-model items. `bynk_check::entry_signature` moves to `bynk_model::entry` (also re-exported at `bynk_model`'s root), with no re-export left in `bynk-check`. The playground's output is unchanged (crate-hygiene B3)
---
