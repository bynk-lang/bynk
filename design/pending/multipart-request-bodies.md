---
level: minor
changelog: `@multipart` decodes a `multipart/form-data` request body into a typed record with `File` parts, under `maxBody`, `maxFile` and `maxParts` caps; `BoundaryRejection` gains `MalformedForm` (#3276, slice 2)
breaking: `BoundaryRejection` gains `MalformedForm(detail)`: a `match` over it with no wildcard stops compiling (`bynk.types.non_exhaustive_match` names the missing variant); add an arm for `MalformedForm(_)`, or a `_` arm.
probes: emit_diagnostics, ts_writes, ts_any, diagnostic_coverage, explained_codes
---

## ADR: multipart-request-bodies
title: A `@multipart` handler annotation decodes a multipart body into a record with `File` parts
summary: The file part type, the caps and buffering, the parser, and the MalformedForm rejection that breaks exhaustive matches

**Context.** `@form` (ADR form-request-bodies) decodes a urlencoded form; a browser file upload still had to be hand-parsed from `Bytes`. This is slice 2 of #3276 (#3180 stays open for streaming a request body).

**Decision.**
- **A: `@multipart`,** parallel to `@form` and `@raw`: one media type, `415` for any other, mutually exclusive with both. Its `body` is a record of form leaves and `File` fields (`Option`, `List`); text parts are read by the `@form` rules, so there is one leaf table.
- **B: the file type.** `File { filename: Option[String], contentType: String, bytes: Bytes }`, a transparent record exported by the `bynk` adapter (`consumes bynk { File }`), not a new builtin: a `File` is plain data a test builds with a record literal. The size is `bytes.length()`. Name and type are client-controlled and documented as untrusted. The proposal called `File` a prelude record that is not a JSON boundary type; as a std record it is an ordinary record to the checker, so the "not a boundary type" refusal (`bynk.types.file_at_boundary`) is **not** implemented in this slice (a handler can pass a `File` across a call, base64-encoding its bytes).
- **C: buffered, capped.** The body is read whole through the counting reader, then parsed. A cap is mandatory (`bynk.http.multipart_body_unbounded`); `maxFile` and `maxParts` (default 64) join `maxBody` in `limits { }` and `@limit`, `maxFile`/`maxParts` in `@limit` only on a `@multipart` route; past either is a `413`. A cap over 32 MiB warns (`bynk.http.multipart_large_cap`): a quarter of a Worker's 128 MB isolate, a judgement and not a measurement.
- **D: the parser.** The platform's `Request.formData()` over the capped bytes. Verified on `workerd` and Node: both parse it, both throw `TypeError` on a malformed body, and they differ on an unchosen file input (`filename=""`: workerd gives an empty string, undici an empty `File`) and on a part with no `Content-Type` (workerd `""`, undici `text/plain`); the reader treats both unchosen-input readings as no file and reads `""` as `application/octet-stream`, and the docs say a part with no type is not something to branch on. Parsed `File` values are laid over the record the codec decoded from placeholders, so file bytes are never base64-encoded and decoded again.
- **E: `MalformedForm`.** A body that is not multipart (no boundary, never closed) is a `400` of kind `MalformedForm`, a new variant of the closed sum `BoundaryRejection`. This is a breaking change, of the kind ADR 0586 took when `@raw` added `PayloadTooLarge` and `UnsupportedMediaType`: a `match` over `BoundaryRejection` that names every variant and has no wildcard stops compiling, with `bynk.types.non_exhaustive_match` naming the missing variant, and needs an arm or a `_`. The in-repo corpus has no such match (the `on rejected` example has a wildcard); a negative fixture pins the diagnostic. The `breaking:` note carries the fix to `reference/upgrading.md`.
- **F: tests.** At `system`, a typed body is sent as a `FormData` (a `Blob` per `File`); a `Wire(…)` body is the multipart text under the boundary `bynk-boundary`. `bynk openapi` documents the body under `multipart/form-data`; a `File` is described as its record, not yet `format: binary`.

**Consequences.** A browser upload can post to a typed route, within a Worker's memory. A large upload still does not fit; streaming a part into `R2.putStream` is the third slice of #3180. Adding `File` to the `bynk` adapter changes every project's emitted `bynk.ts`.
