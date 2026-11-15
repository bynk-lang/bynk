---
level: patch
changelog: A `Postgres` instance runs on `postgres` 4.0.0 (was 3.4.5), so a project with one has its `bynk.lock` row and installed package move to it
breaking: A project with a `Postgres` instance has `postgres` 4.0.0 in its `bynk.lock` and `package.json` now (was 3.4.5). A build run with `--locked` is refused with `bynk.package.lock_out_of_date` ("`bynk.lock` is out of date with the manifests, and `--locked` was given") until you run `bynk fetch` and commit the lock. The driver sends the same statements as before; one visible change is that a parameter whose type the query does not decide (a boolean in a select list with no cast) now reads back as text, so cast it (`$1::bool`).
---
