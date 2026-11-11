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

- **A session in a cookie** — `POST /sessions` mints an unguessable token with
  `Random.bytes(16).toBase64Url()` and returns it with
  `Created("ok").withCookie(Cookie { name: "__Host-sid", value: token, maxAge: Some(30.minutes) })`.
  The `Cookie` defaults are the safe ones (`Secure`, `HttpOnly`, `SameSite=Lax`,
  `Path=/`). `GET /me` and `POST /logout` read it back through `cookies: RequestCookies`, the raw
  view, because a hyphenated `__Host-sid` cannot be a record field; the handler builds its own
  `Token`, which checks the cookie against the same refinement. The `__Host-` prefix binds the
  cookie to this origin, and the compiler checks it: a `Cookie` literal with that name must
  be `secure`, have `path: "/"` and no `domain`. Logout ends the session and clears the
  cookie with `clearCookie`.

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
  ✓ logging in sets a session cookie
  ✓ the cookie the login set identifies its user
  ✓ no cookie means no session
  ✓ logging out clears the cookie and ends the session
  ✓ the http surface reports live sessions

8 passed, 0 failed.
```

## Run it

```sh
bynk dev
```

`bynk dev` compiles, picks the `sessions` worker, and serves it on
`http://localhost:8787` in local mode — the Durable Object is simulated. Then:

```sh
curl -i -XPOST localhost:8787/sessions -d '{"user":"u_42"}'
# HTTP 201, with  set-cookie: __Host-sid=<token>; Max-Age=1800; Path=/; Secure; HttpOnly; SameSite=Lax

curl --cookie '__Host-sid=<token>' localhost:8787/me
# "u_42"

curl localhost:8787/sessions
# 1                                  (live session count)

curl -i -XPOST --cookie '__Host-sid=<token>' localhost:8787/logout
# HTTP 204, clearing the cookie
```

A token resolves to its user until its 30-minute TTL lapses, after which
`GET /me` returns a `401`. (`Secure` cookies are not stored over plain `http`
by browsers other than for `localhost`; `curl --cookie` sends them regardless.)

*Under the hood,* `bynk dev` compiles to `out/workers/sessions/` and runs
`wrangler dev` there. **Deploy** with `npx wrangler deploy` from that directory.
