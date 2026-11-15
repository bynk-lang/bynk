---
level: patch
changelog: A `Postgres` instance runs on `postgres` 4.0.0 (was 3.4.5), so a project with one has its `bynk.lock` row and installed package move to it
breaking: A project with a `Postgres` instance has `postgres` 4.0.0 in its `bynk.lock` and `package.json` now (was 3.4.5). A build run with `--locked` is refused with `bynk.package.lock_out_of_date` ("`bynk.lock` is out of date with the manifests, and `--locked` was given") until you run `bynk fetch` and commit the lock. Bynk keeps postgres.js 3's parameter typing (`infer: true`), so `PgTimestamp`, `PgBoolean` and `PgBytea` bind as documented. One difference remains: a statement with several commands and no parameters (`create table a(…); create table b(…)`) now reports an empty row for each command and a `rowCount` of the commands, where it reported none and 0; a single statement is unchanged.
---
