# Todo

A per-user todo list. The interesting part is the key: the agent is keyed by the
caller's **verified identity**, so every user transparently gets their own
private list with no `userId` parameter to pass — or to forge.

What it shows:

- **An agent keyed by a sealed identity** — `User` is a `Bearer` actor; its
  `UserId` is minted at the boundary and becomes the `Todos` key. `Todos(u.identity)`
  always addresses *the caller's* list.
- **A storage `Map` for state** — items live in `items: Map[String, TodoItem]`,
  read and written with the entry methods `put`/`get`. A `Map` has no implicit
  order, so each item carries a `seq` and the counter `lastSeq` zeroes to `0`.
- **`Query[T]` reads over the map** — `all` is `sortBy(seq).collect()`, `pending`
  is `filter(…).sortBy(…).collect()`, and `pendingCount` is a storage aggregate
  (`filter(…).count()`) that never materialises the list.
- **Tests with no harness** — `bynk test .` constructs the agent by key, calls
  its handlers, and asserts on the scalar results (the aggregate `pendingCount`,
  the created item, the `Result`).

## Layout

```text
todo/
├── bynk.toml
├── src/
│   └── todos.bynk       # context todos — agent + HTTP service
└── tests/
    └── todos.bynk       # tests targeting the todos context
```

## Check and test

```sh
bynk check src
bynk test .
```

```text
todos:
  ✓ add returns the item, freshly created and not done
  ✓ completing a known id succeeds
  ✓ completing an unknown id is NotFound
  ✓ pendingCount falls as items are completed

4 passed, 0 failed.
```

## Run it

```sh
# every request needs an AUTH_JWT_SECRET — supply a local one through the passthrough
bynk dev -- --var AUTH_JWT_SECRET:dev-secret
```

From anywhere inside the project, `bynk dev` compiles, picks the `todos` worker,
and serves it on `http://localhost:8787` in local mode — the Durable Object is
simulated, with nothing to provision first. Then:

```sh
# every request carries a Bearer JWT signed with AUTH_JWT_SECRET; the `sub`
# claim becomes the list owner
curl -XPOST localhost:8787/todos -H "Authorization: Bearer $JWT" -d '{"title":"Buy milk"}'
# {"id":"1","seq":1,"title":"Buy milk","done":false}  (HTTP 201)

curl localhost:8787/todos -H "Authorization: Bearer $JWT"
# [{"id":"1","seq":1,"title":"Buy milk","done":false}]

curl localhost:8787/todos/pending -H "Authorization: Bearer $JWT"
# [{"id":"1","seq":1,"title":"Buy milk","done":false}]   (not-done items, in order)

curl -XPOST localhost:8787/todos/1/complete -H "Authorization: Bearer $JWT"
# (HTTP 204)
```

*Under the hood,* `bynk dev` compiles to `out/workers/todos/` and runs `wrangler
dev` there. **Deploy** with `npx wrangler deploy`; set the real secret with
`npx wrangler secret put AUTH_JWT_SECRET`.
