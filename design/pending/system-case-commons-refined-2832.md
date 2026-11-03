---
level: patch
changelog: A `system` case can call a service whose result is a refined type from a commons the context under test `uses`; the call was typed as `()` and rejected with `bynk.effect.bind_on_non_effect` because the suite's `uses` replaced the target's ([#2832](https://github.com/bynk-lang/bynk/issues/2832))
---
