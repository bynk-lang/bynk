---
level: patch
changelog: `cargo xtask bench` also measures a "linked" project shape (contexts that consume each other, larger handler bodies) and checks the scheduler yardstick, p95 over 250 ms at 200 contexts on either shape, in each release record
---
