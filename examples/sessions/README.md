# Sessions

A live-session store backed by a **`Cache`** — a `Map` whose entries expire on
their own.

What it shows:

- **A `Cache` store with `@ttl`** — `live: Cache[Token, UserId] @ttl(30.minutes)`.
  A `put` (re)starts the entry's lifetime, an entry past its TTL reads as `None`,
  and `size` counts only live entries — expiry without a sweep.
- **Honest time as an effect** — eviction consults the clock, so every cache op
  *except* `remove` declares `given Clock`. The time dependency is visible in the
  signature, and a stubbed clock makes expiry deterministic in a test.
- **Refined boundary types** — `Token` (non-empty, bounded) and `UserId`
  (non-empty) carry their constraints, so a malformed token is rejected at the
  boundary before any cache lookup runs. Those constraints live in
  `commons tokens`.

- **Testing expiry without waiting** — under `bynk test` the platform `Clock`
  is a deterministic test double that reads the epoch. A case moves time on with
  `stub Clock.now() returns each [Instant.fromEpochMillis(0), Instant.fromEpochMillis(1860000)]`:
  the `login` reads the first instant, and the `whoami` 31 minutes later finds
  the session gone.

## Layout

```text
sessions/
├── bynk.toml
├── src/
│   ├── tokens.bynk     # commons tokens — Token + UserId refined types
│   └── sessions.bynk   # context sessions — the Cache-backed agent + HTTP service
└── tests/
    └── sessions.bynk   # tests targeting the sessions context
```

## Check and test

```sh
bynk check src      # type-check, no output
bynk test .
```

```text
sessions:
  ✓ a logged-in token resolves to its user
  ✓ logout ends the session
  ✓ a session expires after its 30-minute ttl
  ✓ the http surface reports live sessions

4 passed, 0 failed.
```

## Run it

```sh
bynk dev
```

`bynk dev` compiles, picks the `sessions` worker, and serves it on
`http://localhost:8787` in local mode — the Durable Object is simulated. Then:

```sh
curl -XPOST localhost:8787/sessions -d '{"token":"s_abc123","user":"u_42"}'
# "ok"  (HTTP 201)

curl localhost:8787/sessions/s_abc123
# "u_42"

curl localhost:8787/sessions
# 1                                  (live session count)

curl -XPOST localhost:8787/sessions/s_abc123/logout
# (HTTP 204)
```

A token resolves to its user until its 30-minute TTL lapses, after which
`GET /sessions/:token` returns a `404`.

*Under the hood,* `bynk dev` compiles to `out/workers/sessions/` and runs
`wrangler dev` there. **Deploy** with `npx wrangler deploy` from that directory.
