# 0497 — The `Bytes` kernel gains concatenation, slicing, a compare method, hex and base64url

- **Status:** Accepted (v0.351)

**Context.** ADR 0142 shipped `Bytes` as a length plus a UTF-8/base64 bridge. Its D4 named
`concat`, `slice`, `fromHex`/`toHex` and lexicographic ordering as follow-ons. Without them a
program can carry binary data but can't join two buffers, take a prefix, print a digest in hex or
read a JWT segment. The standard-library track (`design/archive/retired-tracks.md`, spine #1936)
lists these as kernel gap PR-5, which blocks crypto (K4), binary `Fetch` bodies (K5), KV `Bytes`
values (P1), D1 (P4) and R2 (P5). Slice K1 (#2016) builds them.

**Decision.**

- **D1 — The layer is L0.** Each operation iterates over octets, and Bynk has no loops and no
  octet access to write it with (track §4 rule 1). Each is also synchronous on every platform
  without third-party code, using only typed arrays and `btoa`/`atob` (§4 rule 2, track D5). So
  they are kernel members on `Bytes`, lowered inline or to helpers in the runtime's `bytes.ts`.
  The stable kernel is Foundations-level (`bynk-1.0-definition.md` Gate 1). Kernel members live
  in `bynk-check`'s tables, not the `bynk-std` registry, and carry no stability mark: every kernel
  member is stable by construction. An experimental kernel member would need a stability field
  on `KernelMethod`, and an ADR of its own.
- **D2 — Comparison is a method; `Bytes` stays unorderable.** `a.compare(b) -> Int` returns
  `-1`, `0` or `1` in unsigned lexicographic order: the first differing octet decides, and a
  proper prefix sorts first. `<`, `sortBy`, `min` and `max` on `Bytes` remain type errors. Making
  `Bytes` orderable would route every orderable site (`<`/`<=`/`>`/`>=`, `List.sortBy`,
  `Query.sortBy`, `min`, `max`) through a content compare. A missed site would compile to JS `<`
  on a `Uint8Array`, which compares comma-joined strings: a silently wrong answer, not an error. A
  method keeps the change to one lowering. The kernel has no three-way compare to mirror and no
  `Ordering` type, so the return is `Int`, the host convention. An `Ordering` sum would read
  better in a `match`, but as a new L0 type with one producer it waits for a second one.
- **D3 — `slice` is `String.slice`'s, and copies.** A negative index clamps to `0`, an end past
  the length clamps to the length, and `start >= end` is empty. It never fails. It lowers to
  `Uint8Array.prototype.slice`, which copies, not `subarray`, so a slice never aliases a buffer a
  binding may still hold. `concat` always returns a fresh buffer too.
- **D4 — Hex: lowercase out, either case in, nothing else.** `toHex` emits two lowercase digits
  per octet with no prefix or separators. `fromHex` accepts either case and an even number of
  digits. A `0x` or `sha256=` prefix, whitespace, separators, an odd length or a non-hex character
  is `None`. `""` is `Some(Bytes.empty())`, so `fromHex(b.toHex()) == Some(b)` for every `b`. The
  runtime's private webhook decoder (`__bynkHexToBytes` in `auth.ts`), which tolerates a
  `sha256=` prefix, is a separate thing and is unchanged.
- **D5 — base64url: RFC 4648 §5, unpadded out, padding optional in.** `toBase64Url` uses the `-`
  and `_` alphabet with no padding: the JWT (RFC 7515) form. `fromBase64Url` accepts no padding or
  exactly the padding the length needs. A standard-alphabet `+` or `/`, any other padding or an
  impossible length (`length % 4 == 1`) is `None`. So is non-zero spare bits in the last
  character: `"Zh"` is refused, and only `"Zg"` decodes to `"f"`. That makes the decode canonical,
  with exactly one spelling per value, so a signed token segment can't be re-spelled without
  changing its bytes. The two alphabets stay disjoint: `fromBase64` refuses `-`/`_` and
  `fromBase64Url` refuses `+`/`/`. The JSON wire form of `Bytes` stays standard base64 (ADR 0142
  D5). `fromBase64`, which is also that wire codec's decoder, still accepts non-zero spare bits.
  Tightening it would change what the codec accepts, which is a decision for ADR 0142 and not
  this slice.
- **D6 — Not constant-time.** `compare`, like `==`, stops at the first differing octet. The
  reference says so and points secret comparison at K4's `verify` (track §12).
- **D7 — The helpers are their own import group.** `__bynkBytesConcat`, `__bynkBytesCompare`,
  the hex and the base64url helpers are imported only when called, apart from ADR 0142's group.
  A module that only builds or compares `Bytes` keeps the import line it had, and `slice` needs
  no helper.

**Consequences.**

- `BYTES_METHODS` gains five methods, the `Bytes` statics gain `fromHex` and `fromBase64Url`,
  and completion, hover and signature help pick them up from those tables. No grammar change,
  and no existing emitted output changes.
- Named follow-ons, none needed by K4, K5 or P1–P5: octet access (`get`, `toList`,
  `Bytes.fromList`) with its 0–255 range question, lexicographic orderability, a `b"…"`
  literal, and content-`distinct`.
