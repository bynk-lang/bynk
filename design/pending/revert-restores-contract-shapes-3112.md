---
level: patch
changelog: `bynk revert` keeps a reverted Worker's contract shapes, so a later skew is still classified as compatible or breaking; `bynk.deploy.lock` files shapes by hash in a pruned `contract_shapes` table, and an older lock's per-Worker shapes are moved into it on read
---
