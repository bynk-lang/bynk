---
level: patch
changelog: A `Postgres` instance runs on `postgres` 4.1.1 (was 4.0.0), which fixes the Hyperdrive hang 4.1.0 introduced, so a project with one has its `bynk.lock` row and installed package move to it
breaking: A project with a `Postgres` instance has `postgres` 4.1.1 in its `bynk.lock` and `package.json` now (was 4.0.0). A build run with `--locked` is refused with `bynk.package.lock_out_of_date` ("`bynk.lock` is out of date with the manifests, and `--locked` was given") until you run `bynk fetch` and commit the lock. Nothing else changes: parameter binding, result types and the statement's wire sequence read the same as on 4.0.0.
---
