---
level: patch
changelog: `bynk revert` refuses a version bound to a Hyperdrive config that the ledger has since replaced or pruned, as it already did for KV, D1 and rate limiters (#3970)
---
fix(revert): check a Hyperdrive config's id (#3970)

The deploy facts now record each declared `Postgres` or `MySql` binding's Hyperdrive config id, and `bynk revert`'s `ids` check compares it with the one `bynk.deploy.lock` records, refusing with `bynk.revert.id_unrecorded` when the config has been replaced. An adopted config is checked the same way. Facts written by an earlier deploy have no Hyperdrive id, so a revert to such a version is not refused on this account.
