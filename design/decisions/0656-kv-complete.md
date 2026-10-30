# 0656 — Complete KV with experimental operations beside the existing ones

- **Status:** Accepted (v0.442)

**Context.** `Kv` shipped as text values with `get`, `put`, `putTtl`,
`delete` and a draining `list`. ADR 0050 deferred paging because no Bynk routine
could drive a cursor; R2 has since shipped a paged `list(prefix, cursor)` that a
handler drives a page at a time (P5, #2323). ADR 0051 made TTL a distinct
`putTtl` and kept an options record as the escape hatch "if [options]
cluster". They have: relative and absolute expiry, and metadata. On the
platform, values are bytes, a key is at most 512 bytes, metadata at most 1024
bytes serialised, an expiry at least 60 seconds away, `cacheTtl` at least 30
seconds, and a `list` page at most 1000 keys.

**Decision.**

- **A. Five new operations, experimental; the old ones unchanged.**
  `getWith(key, KvGet) -> Option[KvEntry]`, `putWith(key, value: String,
  KvPut)`, `getBytes(key) -> Option[Bytes]`, `putBytes(key, value: Bytes,
  KvPut)` and `listPage(KvList) -> KvListing`. Each is marked experimental at
  the operation (`bynk.cloudflare.Kv.putWith`, …) inside the stable `Kv`, as
  `Fetch.sendWith` is; the types they use are not gated. `get`, `put`,
  `putTtl`, `delete` and `list` keep their exact behaviour, and no check is
  added to them.
  - **Rejected:** a `KvValue = Text | Binary` sum through `get` and `put`,
    which would change the existing operations; metadata as JSON values, which
    the kernel has no type for (and R2's metadata is `Map[String, String]`).
- **B. Expiry is a sum.** `KvExpiration = Never | After(ttl: Duration) |
  At(instant: Instant)`, extensible. One write cannot carry both a TTL and an
  absolute expiry. A part second rounds up, and an expiry under 60 seconds
  away faults before the write.
- **C. `listPage` pages like `R2.list`.** `KvList = { prefix, cursor, limit }`,
  `limit` 1 to 1000 (`None` for 1000). `KvListing.cursor` is `None` on the last
  page; KV may return a short page that still has a cursor, so a caller loops
  until the cursor is `None`. Each `KvKey` carries its expiry (an `Instant`,
  from KV's epoch seconds) and metadata. The draining `list` stays, so this
  supersedes ADR 0050's deferral without breaking its operation.
- **D. Limits are checked before the call.** A key over 512 UTF-8 bytes,
  metadata over 1024 bytes as JSON, an expiry under 60 seconds away, a
  `cacheTtl` under 30 seconds, and a `limit` outside 1 to 1000 fault the new
  operations. A value over 25 MiB and the per-key write rate are left to the
  platform's error, as for `put`.
- **E. Text and bytes share a namespace.** `getBytes` of a text value gives its
  UTF-8 bytes; reading a binary value as text decodes it as UTF-8, replacing
  invalid sequences. Nothing records which kind a key holds, because the
  platform does not.
- **F. Metadata written outside Bynk** may hold any JSON: a non-string value
  reads back as its JSON text.

**Consequences.** Existing code is untouched. A context can tag entries,
expire them at a time, store binary values and page a large namespace. The
unit double now keeps values as bytes with metadata and expiry (kept and
listed, not enforced) and checks the same limits; the integration double does
the same, and lists a TTL'd key with its expiry, as the platform does.
