---
level: minor
probes: emit_diagnostics, ast_importers, ts_writes, ts_any, diagnostic_coverage, explained_codes
changelog: Inbound email on Workers (`from email`, #3328 slice 2): the `email()` export parses the message with `postal-mime`, dispatches by envelope recipient, mints the `Sender` identity from Cloudflare's DMARC verdict, applies the verdict, and routes via wrangler `addresses`; the first-party npm dependency mechanism of ADR 0799 C lands
---

## ADR: inbound-email-workers-and-first-party-npm
title: Inbound email on Workers, and the first-party npm dependency mechanism
summary: The Worker's email() export, MIME parsing under limits, DMARC verdict selection, fault behaviour, routing, and how a built-in binding brings an exactly-pinned npm package

**Context.** Slice 1 of #3328 (ADR 0805) shipped the surface, the checker and a
runnable suite vertical, and refused a deployable Workers build of a `from email`
service (`bynk.email.workers_unsupported`). Slice 2 is the Workers mapping. It
needs `postal-mime`, which makes it the first consumer of ADR 0799 C (first-party
npm dependencies), a mechanism nothing in the toolchain implemented. This record
covers what slice 2 decides beyond ADR 0805.

**Decision.**

- **A. The first-party npm mechanism (ADR 0799 C), general, not an email special
  case.** `bynk-std` holds one table, `FIRST_PARTY_NPM`: each row is a package, an
  **exact** version (`MAJOR.MINOR.PATCH`, no range operator; a unit test refuses a
  range) and the `@bynk/runtime` module that imports it. A package enters a build
  when that runtime module is *emitted* (the module closure the driver already
  computes), not when a unit is consumed, so a project that never receives mail
  never carries `postal-mime`. The driver writes the pin into `out/package.json`.
  Hyperdrive H1 (#3318) adds `postgres` as a row and a runtime module, and nothing
  else. `bynk.lock` records each row as an `[[npm]]` with `first-party = true`,
  `required-by = ["bynk-std"]`, never `approved`; it is written when the project's own contexts reach the
  binding (`reached_by` in the table: a service of protocol `email` for
  `postal-mime`, decided on the parsed declarations, so a comment or a string that
  spells the words reaches nothing), so a lock records what the project could bring
  into its build and not every pin the toolchain has. H1 adds a `Reach` variant for
  the capability that needs `postgres`. It never reaches `bynk.requires.unapproved` (that check
  covers fetched packages' adapters only) and never appears among `bynk approve`'s
  pending rows (the listing calls it `built-in`). An adapter, in any package, that
  requires the same package at a different version is
  `bynk.requires.conflicting_range`: a project installs one version of each
  package, and the toolchain's pin is the one its binding was tested against. Two
  tests keep the pin honest: it is exact, and it equals the version
  `bynk-emit/runtime/package.json` type-checks and tests the module against.
  Each binding is held to R-12 and R-16 from the retired standard-library track;
  see the security notes below.
- **B. MIME parsing: `postal-mime` 4.1.4.** Pinned exactly; MIT-0; published
  2026-10-22, so it had been public for three weeks when pinned, and its own
  parser limits (`maxNestingDepth`, `maxHeadersSize`, `maxPartCount`,
  `maxRfc822NestingDepth`, `maxForwardedAddresses`) are the ones this module
  sets. The runtime reads the raw stream itself under a byte cap, so a stream that
  lies about its size is cut off at the cap rather than trusted. Limits, all
  enforced: raw size 25 MiB (the platform's cap); 200 MIME parts; nesting depth
  20; 256 KiB of headers over every part; inline `message/rfc822` nesting 3 (deeper
  ones stay attachments); 100 attachments. A message over the size cap is rejected
  `message too large`; one over any other limit, or one the parser cannot read,
  `message could not be read`. Nothing of the message is logged: a log line is a
  fixed string, the address the *service* claims, and a `postal-mime` error code or
  an error's class name, never an error's message (which a handler or the platform
  could build from the message).
- **C. The reply is written by a small first-party writer, not `mimetext`.** A
  reply is one fixed shape: a handful of headers and a text and/or HTML body. The
  writer is about a hundred lines, needs no `nodejs_compat`, and produces: CRLF
  line endings throughout; RFC 2047 `=?UTF-8?B?…?=` words for a non-ASCII subject,
  each at most 75 characters (the code point is never split) and folded; base64
  bodies in 76-column lines; `multipart/alternative` for text plus HTML; a
  numeric-zone `Date` and a generated `Message-ID`. `From` is the receiving
  address and `To` the sender, `In-Reply-To` is the incoming `Message-ID` and
  `References` is the incoming chain (last nine ids) plus it; a message with no
  `Message-ID` is answered with neither. An address containing a character a
  header could be split by, and a control character in a subject, cannot reach a
  header; a threading id that is not a bare `<msg-id>` is not copied. `mimetext`
  (0.7 MB unpacked, with its own dependencies) would add a dependency to review and
  pin for what a test can pin directly, and its header encoding is not ours to
  audit. The writer is exercised by round-tripping its output through `postal-mime`.
- **D. DMARC and the `Sender` identity.** *A refinement of ADR 0805 E:* the verdict
  is read from the **topmost** `Authentication-Results` header only (parsed header
  order, duplicates kept), and only if its authserv-id is `mx.cloudflare.net`. ADR 0805
  said "the first header whose authserv-id is `mx.cloudflare.net`", which searches
  downward; every header below the top can be written by the sender, so whenever
  Cloudflare did not prepend its own, that rule could land on a spoofed one. Now a
  topmost header from any other server, or none, is `DmarcNone`, and the search never
  goes further down. The id is the first token before the first `;`, comments
  stripped, compared case-insensitively and exactly (so
  `mx.cloudflare.net.evil.example` does not match). The verdict is the first `dmarc=`
  result of that header: `pass` and `fail` are those, anything else is `DmarcNone`.
  Tests: a spoofed Cloudflare header below another server's gives `DmarcNone`, as
  does any header below one from another server (property test). The remaining trust
  is Cloudflare's: that Email Routing prepends its header. `SenderIdentity` is
  `{ address: envelope from, dmarc }`. The checker types `s.identity` as
  `SenderIdentity` when the context imports it (`consumes bynk.cloudflare {
  SenderIdentity, … }`), and as the unit value otherwise, as slice 1 did. The
  Worker passes it through the compose wrapper's `__sender` argument into
  `deps.identity`, as `Caller` threads the calling context. A suite has no Email
  Routing: a handler that binds `Sender` reads the envelope sender with `DmarcNone`
  (the case lowering supplies it), so `bynk test` of such a handler runs.
- **E. Faults (ADR 0805 G, now implemented).** Every outcome is settled by the
  runtime and nothing escapes as a throw: a handler fault, or a return that is not a
  verdict, is `setReject("message could not be processed")`; a parse failure is
  `message could not be read`; a `reply` the platform refuses is
  `setReject("reply refused")`; and a `forward` it refuses (a destination that is
  not a verified address) is `setReject("forward refused")`, which ADR 0805 did not
  name. Each is logged without the message.
- **F. Dispatch and routing.** The Worker dispatches on `message.to`, the whole
  address compared case-insensitively, an exact address before a `*@domain`
  catch-all, which claims only that domain (the last `@` decides it). An unclaimed
  recipient is rejected `recipient not accepted` before the stream is read. The
  entry is a short call into the runtime's `handleEmail`. `cloudflare:email`'s
  `EmailMessage` exists only on Workers, so the runtime loads it lazily, when a
  reply is first built (`platformReply`), and a Worker that never replies never
  loads it; tests pass a factory in, and under node a double on `globalThis`
  stands in, so the runtime module runs under node. The emitted `wrangler.toml` lists
  the Worker's addresses (lower-cased, sorted) as the **top-level** `addresses`,
  never inside an `[env.*]`; `bynk deploy` synthesises an `[env.*]` block from
  bindings only, so it is not copied. The runtime is also copied whole into trees
  that never install `postal-mime`, so its import of the package, and of
  `cloudflare:email`, is not type-checked (`@ts-ignore`) and the parsed shape is
  declared in the module; a project that emits the module has the package in its
  `package.json`. The duplicate-address check
  (`bynk.email.duplicate_address`) now also runs project-wide, on the later of two
  contexts (in name order).
- **G. Tests and the system tier.** A `system` case drives an email service by
  posting the message, as raw MIME, to the Worker's real `email()` entry and reading
  back the verdict the entry applied (a `Reply` is read back by decoding the MIME the
  entry wrote). The case's `InboundEmail.headers` become the message's headers, so a
  case chooses the DMARC verdict with an `authentication-results` header. An email
  service is a serialisation edge, so an email-only context qualifies for `system`
  (`bynk.tier.system_needs_wire` now says `http`, `queue` or `email`). Because the
  Worker imports `postal-mime`, `bynk test` installs the emitted `package.json`'s
  dependencies into `out/node_modules` (`npm install --ignore-scripts`) and links them
  into `out-js`, once; a tree that lists none does nothing. A `unit` or `integration`
  case has no Email Routing, so a handler that binds `Sender` reads the envelope sender
  with `DmarcNone`.
- **G2. The retired code.** `bynk.email.workers_unsupported` is removed from the
  registry and `bynk explain`, with its negative fixture and the slice-1 import-closure
  exemption; the reference page keeps a `(historical)` row, as `from websocket`'s
  retired code does.
- **H. The parser boundary is tested, not fuzzed.** `fuzz/` hosts `cargo-fuzz`
  targets for the Rust compiler (`compile`, `parse`); it cannot host a TypeScript
  target. The boundary is covered by seeded property tests in the runtime's own test
  suite (the binding-test style): any mutation of a message, and random bytes, end
  in exactly one outcome and never throw out of `handleEmail`, every log line matches a
  fixed shape and never carries a marker planted in the message, and a header from any
  authserv-id other than `mx.cloudflare.net` is never trusted.

**Security notes (R-16).** The message is untrusted: size is capped by reading, not
by a declared length; every limit above is on the parser's side of the boundary; a
reply is built only from the envelope and the incoming `Message-ID`, never from a
header the sender chose; no log line carries the message. The DMARC rule trusts one
header, by position and authserv-id. The remaining trust is Cloudflare's: that Email
Routing prepends its `Authentication-Results`.

**Missing npm packages fail clearly.** `bynk dev` and `bynk deploy` do not run
`npm install` (that would change every project and is its own decision). Instead, after
the build, they check that every package the emitted `package.json` lists is in
`node_modules` beside it, and stop before wrangler otherwise, naming the packages and
the `npm install` to run in the build directory (so a `from email` project fails with
an instruction, not a bundling error). `--dry-run` only warns (it reports and pushes nothing). Automatic install is
left to a follow-up.

**Routing removals are gated (ADR 0805 F).** Wrangler deletes the rule of an
address a Worker no longer lists, so mail to it bounces. Each Worker's pushed
`addresses` are recorded in the deploy ledger (by environment and Worker,
`email_addresses`, replaced on every push, additive). The next plan computes the
addresses that went; if any did, `bynk deploy` refuses before pushing, naming the
addresses and `--prune`, unless `--prune` is given and confirmed through the
ordinary prune prompt (`--yes` answers it, and `--yes` alone does not). A first
deploy has no record and asks nothing; an added address asks nothing. It does not try
to keep the old address in the pushed config: refusing is enough. The plan lists the
managed addresses (an advisory line, `email_addresses` in JSON) and the removed ones
(`removed_email_addresses`).

**Deferred from this slice.** The `Forward` destination check. The Email Routing
destination-addresses API could not be confirmed from this session (no usable
Cloudflare credential), so the check is left out as the brief says; a `Forward` to an
unverified address is refused by the platform at run time and falls back to
`Reject("forward refused")` (E). The platform behaviour on a *throw* was observed under
`wrangler dev` rather than asserted: an error thrown out of `email()` comes back as a
`500` whose body is the error's message and stack, and is logged as `Uncaught Error:
<message>`, which is why a handler fault is caught and answered with a fixed reason
instead (E). Production behaviour on a throw remains undocumented.

**Consequences.** A `from email` service builds for `--target workers`. A Worker with
one imports `cloudflare:email`, gets `postal-mime` in `out/package.json`, and a
top-level `addresses`. The `bynk.lock` of a project that has one gains a `first-party`
`[[npm]]` row for each of the toolchain's pins.
