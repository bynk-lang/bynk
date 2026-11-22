# Changelog

> **This file is no longer maintained.** The live changelog is
> one page per release under
> [`site/src/content/docs/book/reference/releases/`](site/src/content/docs/book/reference/releases/),
> published as [Releases](https://bynk-lang.org/book/reference/releases/).
> `cargo xtask stamp` writes a row there for every merged increment, from its
> `design/pending/` file. The entries below are kept as history: the Karn → Bynk
> rename and v0.142.0.

## Karn → Bynk rename (historical)

The project, its toolchain, and its in-language surface were renamed from
**Karn** to **Bynk**. This is a **breaking** change for existing sources.

### Toolchain

- The compiler binary `karnc` is now **`bynkc`**; the driver `karn` is now
  **`bynk`** (e.g. `bynk dev`, `bynk test`, `bynk doctor`). The language
  server `karnc-lsp` is now **`bynkc-lsp`**.
- The project manifest `karn.toml` is now **`bynk.toml`**.
- The source file extension `.karn` is now **`.bynk`**.
- `bynk deploy` provisions the KV namespace required by a Cloudflare Worker,
  stores its id in committed `bynk.deploy.lock`, and pushes through Wrangler. It
  supports a non-mutating `--dry-run` plan, JSON output, confirmation / `--yes`,
  and idempotent re-deploys (#583).
- `bynk deploy` ships **every** context of a project, ordered so that each
  Worker is uploaded after the Workers it binds to — Cloudflare resolves a
  Service Binding at upload and rejects one whose target does not yet exist, so
  the order is a correctness requirement. It previously refused any project with
  more than one context. `--context NAME` re-pushes one context and reports
  (rather than pushes into) a dependency that was never deployed. A failure
  stops the run and keeps what landed; a re-run resumes in the same order
  (#601). **Breaking (pre-1.0):** `--format json` describes every context, so
  the top-level `worker`, `kv`, and `deploy` fields are replaced by a `contexts`
  array alongside the resolved `order` — read `.contexts[0].worker` for
  `.worker`, and `.contexts[0].action` (`deploy` or `redeploy`) for `.deploy`,
  which was always `true`.
- `bynk deploy` provisions **every** Cloudflare resource a context declares, not
  only KV. A `service … from queue("n")` has its queue created — by name, before
  the push, because `wrangler deploy` will not create one for you — it checks
  and fails with "To create it, run: wrangler queues create" — and an `agent`'s
  Durable Object migration is applied by `wrangler deploy`. Each queue is
  checked against Cloudflare before every deploy and created only if absent, so
  a queue deleted outside Bynk comes back on the next one. Migration state is **Cloudflare's**, and `bynk.deploy.lock` records
  none of it, so the plan's `migration <tag>` line states what the push will ask
  for and is marked advisory; `bynk deploy` correspondingly cannot report
  migration drift. The plan gains `queue create|reuse <name>` and
  `migration <tag>` lines (in `--format json`, a `queues` array and a
  `migration` object per context). The lock file gains
  `environments.<env>.queues`, a list of created queue names, **additively** —
  an existing lock file loads unchanged, and the version stays `1`. CI's refusal
  to create an unrecorded resource still covers KV alone: a queue is addressed
  by the name in your source, so CI creating one strands nothing (#600).
- `bynk dev` serves **every** context of a multi-context project at once, with
  the Cloudflare Service Bindings between them wired, so cross-context calls
  resolve locally — one `wrangler dev` per context, connected through wrangler's
  dev registry. It previously served one context and failed a multi-context
  project as ambiguous. `--context` is now repeatable and narrows to a subset;
  each context gets its own port from `--base-port` (and its own inspector port
  from `--inspect-port` under `--inspect`), so `--port` is no longer accepted
  through the `--` passthrough where the driver allocates it (#552).

### Formatter

- `bynk fmt` / `bynkc fmt` now honours the 100-column target across the board
  (#963). Every fit test measures the whole line — the column a construct starts
  at, and the signature tail, closing paren or arm comma that follows it — where
  it previously measured the construct alone against the indent, so a body
  behind `fn name(params) -> Ret ` was judged to fit and then overran. Long
  constructs now go vertical: record constructions and list literals one entry
  per line, parameter and argument lists one per line, `&&` / `||` runs broken
  before each operator, `.`-chains broken before each call (unless a trailing
  argument can open its body on the chain's own line), and `if` branches split.
  Reformatting existing sources will produce diffs. The target stays soft — a
  line with no legal break point in it (a long string literal, a `Matches("…")`
  regex) is left long rather than mangled.
- `bynk fmt` / `bynkc fmt` take style flags that override the canonical style
  for one run (#968): `--indent tab|spaces`, `--indent-width N`,
  `--max-line-width COLUMNS`, and `--trailing-comma` / `--no-trailing-comma`.
  Defaults are unchanged, so an invocation that passes none formats exactly as
  before. `--check` judges each file against the style the run asks for, so a
  project on a non-default style gets a working CI gate. Both binaries flatten
  one shared `FmtArgs`, and `bynk fmt` forwards the flags to a `BYNK_BYNKC`-
  pinned compiler rather than silently formatting to the canonical style.
  `--indent-width` alongside tab indentation is an error, not a no-op.

### In-language reserved surface (breaking)

The reserved namespace `karn` is renamed to **`bynk`**. Update your sources:

- `consumes karn { … }` → `consumes bynk { … }`
- platform adapters `karn.cloudflare` / `karn.node` / … → `bynk.cloudflare` /
  `bynk.node` / …
- stdlib units `karn.list` / `karn.map` / `karn.string` → `bynk.list` /
  `bynk.map` / `bynk.string`

### Diagnostics & runtime

- Every diagnostic code is reprefixed `karn.*` → **`bynk.*`** (e.g.
  `karn.namespace.reserved` → `bynk.namespace.reserved`).
- The internal HTTP dispatch prefix `/_karn/` is now **`/_bynk/`** and the
  cross-context caller header `X-Karn-Caller` is now **`X-Bynk-Caller`**.

### Language server

- The editor's project analysis (`bynkc-lsp`, backing diagnostics,
  go-to-definition, and completion) silently omitted five categories of
  whole-project checking it previously reported (P4.2, #1122 — `bynk-ide`
  now reaches `bynk-check`/`bynk-project` directly instead of `bynk-emit`,
  and the new entry point did not yet port these five): `messages`-bundle
  validation, locale-bundle ambiguity, event-subscription validation,
  function-type-boundary checks, and everything inside a `suite`/`test
  integration` body (diagnostics *and* the binding index, so
  go-to-definition/find-references/completion for a test-only binding
  regress too). **`messages`-bundle validation, locale-bundle ambiguity,
  event-subscription validation, and function-type-boundary checks are
  restored** (P5.0/P5.1/P5.2, #1128/#1130/#1126,
  `design/tracks/semantics-in-the-checker.md` §6) — all four now run inside
  `bynk-check::analysis::analyse_project` itself, not just at build time.
  **Everything inside a `suite`/`test integration` body is restored too**
  (P5.4, `design/tracks/semantics-in-the-checker.md` §6) — diagnostics and
  the binding index both, since `process_tests`'/`process_integration_tests`'
  checking half now runs inside `analyse_project` as well
  (`bynk_check::test_suites::phase_test_bodies`/`phase_integration_bodies`),
  so go-to-definition/find-references/completion for a test-only binding
  work again too. Schema-registry reconciliation and platform-lock
  enforcement now originate in `bynk-check` too (P5.3,
  `design/tracks/semantics-in-the-checker.md` §6),
  but are **not** newly affected observably — both were, and remain,
  unreachable from the editor's analysis (the editor never reconciles the
  schema registry, and always analyses as the default Cloudflare
  platform/Bundle target, under which platform-lock can never fire since
  Cloudflare is the only platform-native unit that exists today). A malformed
  `bynk.schema.lock` (`bynk.project.schema_registry_corrupt`) and a
  `Secrets.get` call with a computed argument (`bynk.secrets.computed_name`)
  now originate in `bynk-check` too (P5.5, same track) — also **not**
  newly affected observably, for the same reasons (no on-disk schema lock
  and a Workers-only gate, respectively, neither reachable from the editor's
  hardcoded Cloudflare/Bundle analysis).

### Migrating a project

1. Rename `karn.toml` → `bynk.toml` and every `*.karn` source → `*.bynk`.
2. Replace `consumes karn` with `consumes bynk` and every `karn.<platform>` /
   `karn.<stdlib>` reference with its `bynk.<…>` equivalent.
3. Recompile with `bynkc` (or `bynk build`).

## v0.142.0 — request body-size limits & numeric digit separators

Two features ship together (ADRs 0165 & 0166).

### Request body-size limits for `from http` services (ADR 0165)

A body-taking route (`POST`/`PUT`/`PATCH`) can now bound its request body size:

- A service declares a byte ceiling with a **`limits { maxBody: <Int> }`** section
  in header position (beside `cors { }` / `security { }`), overridable per route
  with a **`@limit(maxBody: <Int>)`** handler annotation (the `@cache` placement;
  valid **only** on `POST`/`PUT`/`PATCH`).
- A capped route rejects a request whose `Content-Length` exceeds the cap with a
  synthesised **`413 PayloadTooLarge`** (`{ kind: "PayloadTooLarge", details: … }`),
  produced **before the body is read** and before the `by`/Bearer auth seam — the
  boundary posture of the method-semantics `405`. It reuses the existing `413`
  status, so the closed `HttpResult` registry is unchanged; the `413` is
  CORS/security-header-stamped so a cross-origin caller can read it.
- Precedence is **route `@limit` → service `limits` → none**. With neither, a route
  has **no cap** and emits byte-for-byte unchanged output — the feature is opt-in.
- Enforcement is a `Content-Length` fast-reject (not a hard guarantee — the header
  can be absent for a chunked transfer or spoofed), pairing with the Workers
  platform cap. A streamed-read cap is a named follow-on.
- `maxBody` is a positive `Int` byte count (`26_214_400` for 25 MiB). A byte `Size`
  literal (`1.mb`) is a named follow-on.

### Numeric digit separators (ADR 0166)

`Int` and `Float` literals now admit an `_` digit separator between digit groups
(`1_048_576`, `1_000.5`) — never leading, trailing, or doubled. The separators are
stripped before the value is parsed (purely visual), and the as-written lexeme is
preserved so `bynk fmt` keeps the author's grouping. Motivated by `maxBody`'s large
byte counts, but applies language-wide.

Closes #494.
