---
title: Events
---
A context declares an **`event`** it may emit, `given Events` emission raises
one at handler commit, and another context's service subscribes with **`from
Events(E)`**. Delivery is fire-and-forget across contexts — the emitting
handler never learns whether, or how, a subscriber ran.

**Understand**
- [Understand events](/book/guides/events/understand-events/)

**Worked example:** [`bynk-tests/tests/events_behaviour.rs`](https://github.com/bynk-lang/bynk/blob/main/bynk-tests/tests/events_behaviour.rs)
— a publisher, a subscriber, and the release-at-commit/abort-suppression proof,
compiled and run under real `tsc` + `node`.

**See also:** [First-party `bynk` capabilities](/book/reference/bynk-capabilities/),
[Understand the capability model](/book/guides/effects-and-capabilities/understand-the-capability-model/),
[Diagnostics](/book/reference/diagnostics/).
