---
level: patch
changelog: A selective `consumes` no longer lets the standard-library types it did not list clash with a suite's types or make a context's own variant ambiguous (`Claim`, `Claimed`, `ContainerError`'s `Unavailable`)
probes: diagnostic_coverage
---
