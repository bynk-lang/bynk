---
level: minor
probes: ts_writes
changelog: "`String` gains `percentEncode()` and `percentDecode() -> Option[String]` (RFC 3986, over the UTF-8 bytes), and a new `bynk.url` commons, written in Bynk, adds a `Url` record with `parse`/`format`, `parseQuery`/`formatQuery` and helpers (standard-library K2, [#2594](https://github.com/bynk-lang/bynk/issues/2594))"
---

## ADR: url-commons
title: URLs as a pure-Bynk commons over two percent-encoding kernel methods
summary: `Url` is a record in a `bynk.url` commons written in Bynk (RFC 3986 `parse`/`format`, query-string helpers), not a base type, because a base type's name is a lexer token; only percent-encoding is kernel, since it needs a string's UTF-8 bytes

**Context.** Track §6.1 lists URL parse and build, query strings and
percent-encoding as missing; a Fetch caller built query strings by `concat`
with no escaping. Track rule 1 admits a kernel addition only when it is
irreducible, and leaves everything else to L1 (ADR 0034/0036). The base types
`Bytes`, `Instant` and `Duration` are lexer tokens, so a base type named `Url`
would make `Url` a keyword and break every user `type Url`, the shipped
link-shortener example's included. Commons units are pure Bynk with no native
binding. The `String` kernel (`indexOf`, `slice`, `split`, `chars`, …) and
`Int.parse` are enough to write URL structure in Bynk; they cannot reach a
string's UTF-8 bytes.

**Decision.**

- **A. Two kernel methods, because they are irreducible.** `s.percentEncode()
  -> String` keeps RFC 3986's unreserved set and writes every other UTF-8 byte
  as `%XX` in upper-case hex (so `! ' ( ) *` too, unlike
  `encodeURIComponent`; a lone surrogate as U+FFFD). `s.percentDecode() ->
  Option[String]` reads `%XX` in either case and is `None` for a malformed
  escape or bytes that are not UTF-8; `+` stays `+`. They live in a new `url`
  runtime module, imported only by a module that calls them.
- **B. `Url` is a record in a commons.** `bynk.url` declares `QueryParam` and
  `Url` (`scheme`, `host`, `port`, `path`, `query`, `fragment`) and is used with
  `uses bynk.url`, so a project's own `Url` is untouched unless it opts in. A
  record is inspectable, codable on the wire, and updatable with a spread.
  **Rejected:** a base type (a lexer keyword, breaking); an opaque value (needs
  a kernel type for the same reason).
- **C. RFC 3986, strictly, in Bynk.** `parse` reads
  `scheme://host[:port][path][?query][#fragment]`: scheme and host lowered, the
  path kept as written, the query split on `&` and each pair on its first `=`,
  `+` read as a space, then percent-decoded. A user-info part, an IPv6 literal,
  a missing `//`, a port over 65535, a malformed escape anywhere, or a path
  character RFC 3986 does not allow unencoded is `None`; nothing is repaired. `format` writes it back, and `parse(format(u))` is `Some(u)` for
  every `u` that `parse` returned. **Rejected:** WHATWG parsing through the
  platform's `URL`, which needs a kernel static returning this commons' record
  (new checker machinery) and normalises input a request-building library
  should refuse.
- **D. Query strings stand alone.** `formatQuery`, a lenient `parseQuery`
  (a leading `?` dropped, an undecodable pair skipped) and a strict
  `decodeQuery`, plus `param`, `params`, `withParam`, `withoutParam` and
  `segments`.
- **E. Every helper is exported.** A commons has no private functions, so the
  pieces `parse` is built from (`isScheme`, `isHost`, `isPath`, `parsePort`,
  `decodePair`) are public and documented as such.
- **F. Stable**, like K1's additions.

**Consequences.** A service can build an escaped query string in one call and
read a URL into a record. `Request.url` stays a `String` (K5a: `Request` cannot
grow without breaking literals). WHATWG semantics, relative URLs and IPv6
hosts are out of scope.
