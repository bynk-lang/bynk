---
level: patch
changelog: The Rust token signer is byte-compared with the test runtime's signer for non-ASCII subjects
---

Test-only: #2877's `TextEncoder` fix landed with #2864; this widens the #2869
byte-comparison vector to non-ASCII subjects. The `system` case
`by Reader("ünï")` already lives in `2864_system_bearer_refusals`.
