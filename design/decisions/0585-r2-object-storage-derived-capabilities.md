# 0585 — R2 object storage as instance-only derived capabilities

- **Status:** Accepted (v0.390)

**Context.** A Bynk service had no object storage: Kv holds `String` values up
to 25 MiB. The first-party registry (#2308) stores package archives (immutable,
content-addressed, up to 64 MiB) and rewritten index files. Standard-library
slice P5 (track §9) asked for R2 with byte streams, metadata, conditional
operations and the physical-name scope confirmed. S4a/S4b built derived
capabilities and ledger provisioning for Kv; K1 gave `Bytes` its operations.

**Decision.**

- **A. Instance-only.** `R2` joins `bynk.cloudflare`'s instantiable kinds with
  prefix `R2` and `bare: false`, like `Service`; its one creation option is
  `@jurisdiction("eu" | "fedramp")`. A bare `given R2` is
  `bynk.capability.instance_only`; an unlisted option is
  `bynk.capability.unknown_option`, an unlisted jurisdiction
  `bynk.capability.invalid_option`. D3's implicit default instance exists for
  Kv's back-compatibility, which R2 has none of.
- **B. Whole objects; byte streams split out.** `get` and `put` move whole
  `Bytes`, buffered in the Worker. This re-scopes track row P5: `Stream[Bytes]`
  across the boundary is language work (ADR 0023), so it becomes a K-slice plus
  a P5b (streamed `get`/`put`), filed when a consumer needs objects too large to
  buffer. The registry's 64 MiB cap fits a 128 MB isolate.
- **C. Operations.** `get`, `head`, `put`, `putWith`, `delete`, `list(prefix,
  cursor)`. `putWith` takes `R2Put { contentType, metadata, onlyIf }` and gives
  `None` when the condition fails. `onlyIf` is `R2Condition`:
  `Always | Absent | EtagMatches(etag)`. Declared as a sum, not the proposal's
  `enum { …, EtagMatches(String) }`, because Bynk's `enum` has no payloads; the
  payload field is named `etag`. `Absent` is R2's `etagDoesNotMatch: "*"`,
  atomic with the write (verified on workerd). `list` returns one page of up to
  1000 objects (`limit: 1000` is passed explicitly: a local bucket otherwise
  pages at 100) and is never drained; a page may hold fewer than the limit, so
  the cursor alone says whether more remain.
- **D. Deploy.** The bucket is named `<worker>-<capability, kebab>`,
  environment-qualified (`env_qualify`), emitted as `[[r2_buckets]]
  bucket_name` (plus `jurisdiction`) and recorded in the ledger's
  `r2_instances` (name and jurisdiction; there is no minted id). Provisioning is
  `wrangler r2 bucket create <name> [--jurisdiction …]`, an existing bucket we
  own being success. A derived name outside R2's rules (3–63 characters,
  lowercase letters, digits, hyphens, no leading or trailing hyphen) and two
  instances sharing a name are plan-time errors. A renamed or removed instance
  is a reported orphan, never deleted by `bynk deploy` or `--prune`. A changed
  `@jurisdiction` plans `recreate` (a new bucket; the old one and its objects
  stay) and refuses without `--yes`. `dev --remote` binds recorded buckets under
  the environment's name and refuses an unprovisioned one.
- **E. Tests at every tier.** An in-memory double per instance at the unit tier
  (`__bynkTest_<X>()`, honouring conditions, metadata and paging) and
  `__makeIntegrationR2()` at the integration tier; the binding is tested against
  a contract-faithful fake and round-tripped on Miniflare under
  `BYNK_REQUIRE_WORKERD`.
- **F. Experimental.** `R2` is in `Unit.experimental` (opt-in through
  `[unstable] allow = ["bynk.cloudflare.R2"]`) and `R2Condition` is extensible,
  until the first-party registry has run on it in production (#2308 R6).

**Account scope (track §11 Q3).** R2's bucket naming rules (lowercase letters,
digits, hyphens; 3–63 characters; no leading or trailing hyphen) are those of
Cloudflare's "Create buckets" page,
<https://developers.cloudflare.com/r2/buckets/create-buckets/>. That page does
not state the uniqueness scope, so this slice does not rest on it: names are
treated as account-scoped, the conservative reading, so a name clash is refused
across the project and across the ledger's environments rather than discovered
at `bucket create`. A bucket is addressed by `(name, jurisdiction)` on an
account, which is why a changed jurisdiction is a new bucket.

**Consequences.** The registry's read and write paths (#2308 R1, R2) can be
built. Objects larger than a Worker can buffer need the follow-on. The types
`R2Object`, `R2Put`, `R2Condition` and `R2Listing` are named with `consumes
bynk.cloudflare` beside the `{ R2 }` selection. Orphaned buckets stay in the
ledger and the plan until the entry is removed by hand once the bucket is dealt
with. The emitter now imports an adapter's types that appear only in a checked
type (previously treated like a consumed context's and skipped).
