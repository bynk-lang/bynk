---
level: minor
changelog: `~>` is deprecated for `do e.detach()`: a warning with a `bynk check --fix` rewrite, and `~>` now lowers through the same failure-reporting helper
probes: diagnostic_coverage
---

Release N of the `~>` retirement in #3437 (accepted). `~> e` keeps its checks (an `Effect[()]` operand, the agent-handler refusals) and now warns, `bynk.send.deprecated`, with a machine-applicable rewrite to `do e.detach()` that `bynk check --fix` applies. A send that is refused is not also warned about. `~>` lowers through `__bynkDetach`, so its faults are labelled with the site and are reported. Warnings denied in a build fail it until the rewrite is applied. The repository's own corpus is rewritten in the next change; `~>` is removed at the following release.
