# Greeter, i18n

A greeter that replies in the caller's own language — negotiated for real from
the `Accept-Language` header, against a small message bundle.

What it shows:

- **The `Locale` capability** — `consumes bynk { Locale }` and
  `Locale.current()`. On Cloudflare, the platform provider negotiates
  `Accept-Language` against `greeter.messages`' declared locales (RFC 4647
  basic filtering: exact match, then rightmost-subtag truncation, falling
  back to the reference locale) — no negotiation code in this project at all.
- **A message bundle** — `messages en @reference { ... }` plus `fr` and `es`
  blocks. The checker requires every reference-locale code to reappear, with
  the same placeholders, in every other declared locale.
- **`bynk.locale.types`** — `Locale.current()`'s `LocaleTag` and the bundle's
  own `render`/`message` API are reached through two separate, narrow `uses`
  clauses (`bynk.locale.types` in the context; `bynk.locale` inside the
  message-bundle commons only) rather than one broad import — see *Where to
  go next* below.

## Layout

```text
greeter-i18n/
├── bynk.toml
├── src/
│   └── greeter/
│       ├── messages.bynk   # commons greeter.messages — the bundle + greet()
│       └── web.bynk        # context greeter.web — the HTTP service
└── tests/
    └── greeter/
        └── messages.bynk   # tests targeting greeter.messages
```

## Check and test

```sh
bynk check src
bynk test .
```

```text
greeter.messages:
  ✓ greets in the reference locale
  ✓ greets in a declared second locale
  ✓ greets in a declared third locale
  ✓ falls back to the reference for an undeclared locale
  ✓ farewells too, not just greetings

5 passed, 0 failed.
```

`greeter.messages` (the bundle and its `greet` wrapper) is unit-tested by
passing locale tags as literals — no `Locale` binding needed. `greeter.web`
consumes the platform capability, which `bynk test` replaces with a test double
that answers `"en"` (or whatever a `stub Locale.current()` says). Exercise it end
to end under `bynk dev`, below.

## Run it locally

```sh
bynk dev
```

From anywhere inside the project, `bynk dev` compiles, picks the
`greeter-web` worker, and serves it on `http://localhost:8787`. Then, in
another terminal:

```sh
curl http://localhost:8787/hello/Ada
# "Hello, Ada!"                         (no header — falls back to the reference)

curl -H "Accept-Language: fr" http://localhost:8787/hello/Ada
# "Bonjour, Ada !"                      (exact match)

curl -H "Accept-Language: es-MX" http://localhost:8787/hello/Ada
# "¡Hola, Ada!"                         (es-MX truncates to its declared es)

curl -H "Accept-Language: de" http://localhost:8787/hello/Ada
# "Hello, Ada!"                         (de isn't declared — falls back to en)

curl -H "Accept-Language: fr" http://localhost:8787/bye/Ada
# "Au revoir, Ada !"
```

*Under the hood,* `bynk dev` runs the manual recipe:

```sh
bynk compile src --output out --target workers
cd out/workers/greeter-web
npx wrangler dev
```

## Deploy it

```sh
cd out/workers/greeter-web
npx wrangler deploy
```

No provisioning needed — `Locale` has no external binding to create.

## Where to go next

- [`hello-world`](../hello-world/) — refined types, capabilities, typed HTTP,
  from first principles.

`LocaleTag` and the bundle's `render`/`message` API live in two separate
commons — `bynk.locale.types` (imported by `greeter.web`) and `bynk.locale`
(imported only by `greeter.messages`, which needs the builder functions) —
rather than one. A context needing both `Locale.current()` and a message
bundle in the same scope would otherwise hit a `uses`-clause name collision
on `render`; the split exists specifically so a project shaped like this one
compiles at all.
