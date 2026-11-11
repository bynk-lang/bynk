# 0810 — `Form.decode` and `Form.encode` expose the `@form` mapping as a codec

- **Status:** Accepted (v0.555)

**Context.** `@form` (#3276, slice 1) decodes a form body at the router, and the system-tier driver already writes one. A `@raw` handler, a signed webhook that must keep the octets it verified, and an outbound `Fetch.sendBytes` had no way to use that mapping. Decision G of the accepted proposal exposes it.

**Decision.**
- **A: a `Form` namespace beside `Json` and `Yaml`.** `Form.decode[T](s) -> Result[T, DecodeError]` and `Form.encode(v) -> String`. `T` is inferred from an expected `Result[T, DecodeError]`, as for `Json.decode`. There is no `Multipart.decode`: a `File` part has no useful text form.
- **B: one mapping.** `Form.decode` runs the router's own reader (`formObject`, the `fieldsObject` leaf table behind `query`, `@form` and `cookies`) over the text, then the record's typed codec, so refinements, defaults and `Option`-for-absent hold exactly as at the boundary. `Form.encode` is the driver's `formString` over the record's wire value. A `decode` failure is a `DecodeError` rooted at `$`, as `Json.decode`'s is; the router's `400` roots at `body`.
- **C: the target is a record of form fields.** The same field set as a `@form` body (`bynk.types.form_uncodable` otherwise), checked at the call from the record's declaration.

**Consequences.** A service can verify a webhook's signature over the raw form octets and then decode them, or send a form to another service, with the codec the router uses. The field table is built from `T` at the call site, so a `Form` call costs one table literal per use. A `Option` field's path in an error is `$.age.value`, as `Json.decode` writes it, not the router's `body.age`.
