---
level: patch
changelog: A Worker's D1, R2, Analytics Engine and Vectorize resource names, and where its D1 migrations are staged, come from the `WorkerLayout` rather than the context name, with the identity layout unchanged (#3562, G2b step 2c-2)
---

No user-visible change: without a `[deploy.workers]` table every resource name
and staged path is byte-identical. The migrations' check side
(`declared_databases`, `D1Sql`), `wrangler.toml`'s resource blocks and the
driver's staged migrations now take their Worker identity from one
`WorkerLayout`, built for a project by `WorkerLayout::for_project`, so they
cannot disagree once a group renames the Worker. A driver test pins that, for a
grouped layout, the database name, `migrations_dir` and staged path agree.
