---
level: minor
changelog: The request readers apply `@name("…")`: a renamed `cookies`, `query` or `@form`/`@multipart` body field is read from, and written to, its wire key (#3343, slice 2)
---

## ADR: field-name-readers
title: The request readers apply a record field's `@name` wire key
summary: Where a renamed field is read, written and documented, and what an error names

**Context.** `@name("…")` on a request record's field (ADR from slice 1) was checked but not applied: the readers still looked for the field's own name, so `@name("__Host-sid") sid` could not read a `__Host-sid` cookie.

**Decision.** One reader reads every request surface (`fieldsObject`), so the wire key is a single optional `wire` on its `QueryField`; absent, the key is the field name. The router, the typed system-test drivers (`__queryString`, `__cookieString`, `__multipartForm`, and the form body) and `Form.decode`/`Form.encode` take the same table, so a record is written under the keys it is read from. A rejection still names the field (`query.pageSize`), not the key, because the field is what the author wrote. The OpenAPI document lists a renamed `cookie`/`query` parameter, and a renamed form body's properties, under their wire names; hover shows the wire name with its field. Records without a rename emit no table and are unchanged.

**Consequences.** A prefixed cookie such as `__Host-sid` is readable through the typed form; `examples/sessions` can move to it in the closing slice. The generated emission of an un-renamed record is byte-for-byte what it was.
