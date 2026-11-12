---
level: patch
changelog: The repository's own corpus, spec §5.5, the guides and the reference describe `e.detach()` and the deprecated `~>`
---

Release N of the `~>` retirement in #3437 (accepted): the fixtures that used `~>` are rewritten to `do e.detach()` with `bynk check --fix` (the refusal fixtures and the `221_async_send_waituntil` deprecation fixture keep `~>`), spec §5.5, §4.8.5, §7.3.2 and the new §7.4.17, the *Synchronous and asynchronous sends* guide, the testing, capabilities, operators and grammar references and the emission notes now describe `detach` and the failure route, and the retired-tracks note points to #3437.
