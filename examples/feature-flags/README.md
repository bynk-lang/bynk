# Feature flags

A tiny feature-flag service: **anyone may read** a flag, **only an editor may
write** one. The flags live in Workers KV as JSON.

What it shows:

- **Two actors on one service** — public reads declare `by Visitor`; writes
  declare `by e: Editor`, a `Bearer` user carrying an `editor` claim. A missing
  token is `401`, a non-editor token `403`, both enforced at the boundary.
- **The KV surface** — `get` / `put` / `delete`, plus `list(Some("flag:"))` to
  enumerate keys by prefix.
- **The typed JSON codec** — flags are stored with `Json.encode` and read back
  through `Json.decode[Flag]`; a corrupt value surfaces as a `500`, never a
  silent `undefined`.
- **List combinators** — the `List` method `keys.map(f)` strips the key prefix
  back to bare flag names.

## Layout

```text
feature-flags/
├── bynk.toml
├── src/
│   ├── keys.bynk      # commons keys — FlagKey + key helpers
│   └── flags.bynk     # context flags — the HTTP service
└── tests/
    └── keys.bynk      # unit tests for the key helpers + boundary
```

## Check and test

```sh
bynk check src
bynk test .
```

```text
keys:
  ✓ keyOf namespaces a flag name
  ✓ nameOf is the inverse of keyOf
  ✓ an empty flag name is rejected at the boundary
  ✓ a 64-character name is accepted, 65 is rejected

4 passed, 0 failed.
```

The `FlagKey` boundary and the `keyOf`/`nameOf` helpers live in `commons keys`,
so they are unit-tested without a KV binding. The HTTP handlers themselves
consume the platform `Kv`, which is in memory under `bynk test`. Exercise them
end to end under `bynk dev`, below.

## Run it

```sh
# writes need an AUTH_JWT_SECRET — supply a local one through the passthrough
bynk dev -- --var AUTH_JWT_SECRET:dev-secret
```

From anywhere inside the project, `bynk dev` compiles, picks the `flags` worker,
and serves it on `http://localhost:8787` in local mode — KV is simulated, so
there's nothing to provision first. Then:

```sh
# public read
curl localhost:8787/flags
# []  (nothing yet)

# write requires an editor JWT signed with AUTH_JWT_SECRET and an "editor" claim
curl -XPUT localhost:8787/flags/new-dashboard \
  -H "Authorization: Bearer $EDITOR_JWT" \
  -d '{"enabled":true,"description":"the redesigned dashboard"}'

curl localhost:8787/flags/new-dashboard
# {"enabled":true,"description":"the redesigned dashboard"}

curl localhost:8787/flags
# ["new-dashboard"]
```

*Under the hood,* `bynk dev` compiles to `out/workers/flags/` and runs
`wrangler dev` there. To **deploy** for real: `npx wrangler deploy`, set the real
secret with `npx wrangler secret put AUTH_JWT_SECRET`, and create the KV
namespace (`npx wrangler kv namespace create KV`, then paste the id into
`wrangler.toml`).
