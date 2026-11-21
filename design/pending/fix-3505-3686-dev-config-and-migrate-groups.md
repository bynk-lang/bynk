---
level: patch
changelog: `bynk dev` serves each Worker from a copy of its config that is replaced only once the development bindings are in it, so a rebuild never briefly binds the deploy's Vectorize index, and a Worker with stand-ins gets its development index too; `bynk migrate status` and `bynk migrate sweep` find the agent classes of a context in a `[deploy.workers]` group in the group's Worker
---
