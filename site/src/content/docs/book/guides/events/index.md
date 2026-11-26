---
title: Events
---
A context declares an **`event`** it may emit, `given Events` emission raises
one at handler commit, and another context's service subscribes with **`from
Events(E)`**. Delivery is at most once today, and the emitting handler never learns
whether, or how, a subscriber ran (on Workers it does wait for delivery to
finish). See [Understand events](/book/concepts/events/understand-events/).

**Understand**
- [Understand events](/book/concepts/events/understand-events/)

**Do**
- [Rename a field safely](/book/guides/type-system/rename-a-field-safely/)

**Worked example:** [`bynk-tests/tests/events_behaviour.rs`](https://github.com/bynk-lang/bynk/blob/main/bynk-tests/tests/events_behaviour.rs)
— a publisher, a subscriber, and the release-at-commit/abort-suppression proof,
compiled and run under real `tsc` + `node`.

**See also:** [First-party `bynk` capabilities](/reference/bynk-capabilities/),
[Understand the capability model](/book/concepts/effects-and-capabilities/understand-the-capability-model/),
[Diagnostics](/reference/diagnostics/).
