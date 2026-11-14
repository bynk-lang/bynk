---
level: patch
changelog: Inbound email parses with `postal-mime` 4.2.1 (was 4.1.4), so a project with an email service has its `bynk.lock` row and installed package move to it
breaking: A project with an inbound email service has `postal-mime` 4.2.1 in its `bynk.lock` and `package.json` now (was 4.1.4). A build run with `--locked` is refused with `bynk.package.lock_out_of_date` ("`bynk.lock` is out of date with the manifests, and `--locked` was given") until you run `bynk fetch` and commit the lock.
---
