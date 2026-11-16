---
level: patch
changelog: Two members of one `[deploy.workers]` group that serve a fixed asset path from different files, or at paths one of which would be a folder of the other, are refused (`bynk.deploy.asset_path_conflict`); the same file reached by both members is fine (#3562, G2b)
probes: diagnostic_coverage, emit_diagnostics, explained_codes
---

A group has one assets directory and one `[assets]` block, so its members' assets
merge: distinct paths all ship, and a file two members reach through a shared
commons is one file. Only a fixed path (`at "/favicon.ico"`) that two members
claim from different files, or one that would be a folder of the other's, has no
single answer. The check names both members and reports at the later member's
declaration; a clash inside one member stays `bynk.asset.path_conflict`.
Content-hashed paths need no check, since one path is one content.

Also in this change: the `bynk.deploy.multiple_websocket_members` explanation
says the one websocket member takes every upgrade whatever the path, and the
group dispatch test runs `scheduled`, `queue` and `email` as well as `fetch`.
