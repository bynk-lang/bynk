---
level: patch
changelog: Inbound email parses with `postal-mime` 4.2.1 (was 4.1.4), so a project with an email service has its `bynk.lock` row and installed package move to it
breaking: A project with an inbound email service has `postal-mime` 4.2.1 in its `bynk.lock` and `package.json` now (was 4.1.4); `bynk fetch` rewrites the lock, but a build run with `--locked` is refused as out of date until you run `bynk fetch` and commit the lock.
---
