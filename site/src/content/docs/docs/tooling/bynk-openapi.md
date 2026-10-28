---
title: "Publish your service's OpenAPI"
---
`bynk openapi` writes an [OpenAPI 3.1](https://spec.openapis.org/oas/v3.1.0)
document for a project's `from http` services. It is the compiler publishing the
contract it already enforces: the routes, the handler params, each boundary
type's shape and refinements, and the exact status codes each handler can return
are all facts the compiler holds, and a generated document cannot drift from the
handlers the way a hand-written one does.

The document serves API consumers, client code generation, docs publishing and
CI. It is read-only: nothing is written into the build, and the normal `.ts`
output is unchanged.

## A recipe

From the project root, for the [rate limiter](https://github.com/bynk-lang/bynk/tree/main/examples/rate-limiter):

```sh
bynk openapi > openapi.json            # stdout composes in CI
bynk openapi --out openapi.yaml        # a .yaml/.yml extension picks YAML
bynk openapi --service api             # one service of several
```

For `GET /check/:client`, with `type ClientId = String where NonEmpty`, the
output includes:

```json
"/check/{client}": {
  "get": {
    "parameters": [{
      "name": "client", "in": "path", "required": true,
      "schema": { "type": "string", "minLength": 1, "x-bynk-refinements": ["NonEmpty"] }
    }],
    "responses": {
      "200": { "content": { "application/json": { "schema": { "$ref": "#/components/schemas/RateView" } } } },
      "400": { "…": "the boundary's rejection, BynkBoundaryError" },
      "429": { "…": "{ \"error\": string }" }
    }
  }
}
```

The project must check clean first. Run the output through any OpenAPI 3.1 tool:
a client generator, a documentation renderer, or a linter in CI.

## What becomes what

| Bynk | OpenAPI |
|---|---|
| `on GET("/check/:client")` | `paths["/check/{client}"].get`, the `:segment` becoming `{segment}` |
| a path param | a `parameters` entry with `in: path`; a refined type's constraints sit on the parameter's own schema |
| the `body` param | `requestBody` (`application/json`; a `@raw` route takes `format: binary` under its declared media type) |
| `HttpResult[T]` success, `Created(t)`, `Accepted(t)` | a `2xx` response whose body is `T` |
| `TooManyRequests(msg)` and the other message variants | the status, with body `{ "error": string }` |
| redirect variants | the status, with a `Location` header |
| the boundary's own rejection | `400`, described once as `BynkBoundaryError` |
| a record | `type: object`, its fields as `properties`, fields with a default omitted from `required` |
| a sum | `oneOf`, one object per variant with `kind` set to the variant's name |
| `Option[T]`, `Result[T, E]` | `oneOf` of the `Some`/`None` and `Ok`/`Err` objects the codec writes |
| `List[T]`; `Map[K, V]` | `array`; an array of `[key, value]` pairs |
| `Bytes` | `string` with `contentEncoding: base64` |
| a named type | one entry in `components/schemas`, shared by `$ref` |

The responses are the handler's, not the whole `HttpResult` vocabulary: the
declared success appears only if the body returns it, and each error variant only
if the body constructs it.

## Refinements

Every predicate a boundary type carries is one the codec re-validates, and each
maps to a JSON Schema keyword. Each refined schema also carries the predicates as
written in `x-bynk-refinements` and spells them out in its `description`, so a
consumer reading only the extension, or only the prose, sees the whole check.

| Predicate | JSON Schema |
|---|---|
| `NonEmpty` | `minLength: 1` |
| `MinLength(n)` / `MaxLength(n)` | `minLength: n` / `maxLength: n` |
| `Length(n)` | `minLength: n`, `maxLength: n` |
| `Positive` | `exclusiveMinimum: 0` |
| `NonNegative` | `minimum: 0` |
| `InRange(a, b)` | `minimum: a`, `maximum: b` |
| `Matches("p")` | `pattern: "^(?:p)$"` — the codec matches the whole value, so the pattern is anchored |

An `opaque` type adds `x-bynk-opaque: true`. `Instant` and `Duration` carry
`x-bynk-base`. Cross-field predicates (`implies`) belong to agent invariants, not
to the types that cross the wire, so none appears in a document.

## Not yet

`bynk openapi --check`, to diff the output against a committed baseline and gate
breaking changes, and an in-editor service explorer that renders the document and
invokes endpoints, are follow-ups. So are security schemes from `by` clauses and
the agent, queue and WebSocket boundaries.
