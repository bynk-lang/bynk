//! #1969: renders `reference/wire-format.md`, the published reference for wire
//! format version 1.
//!
//! Every fact the page states that the compiler also uses is read from the
//! compiler: the version, headers and check order from [`super`], the base-type
//! encodings from [`crate::wire`], and the contract hash from
//! [`crate::contract`], computed on [`EXAMPLE_SOURCE`]. The page's examples
//! ([`EXAMPLES`]) are executed: `bynkc/tests/wire_reference.rs` compiles
//! [`EXAMPLE_SOURCE`] to Workers, runs every example through the generated
//! codecs on Node, and checks the emitted hashes and dispatch against this page.
//! Regenerate the page with
//! `BYNK_BLESS=1 cargo test -p bynkc --test it wire_reference::`.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use bynk_syntax::ast::{BaseType, CommonsItem, SourceUnit, TypeDecl};

use super::{
    ACCEPTED_WIRE_VERSIONS, CALL_CHECKS, CALL_METHOD, CALL_PATH_PREFIX, CALLER_HEADER,
    CONTRACT_HEADER, EDGE_FROM_PREFIX, EDGE_TO_PREFIX, HEADERS, MAX_BODY_BYTES, SIGNATURE_HEADER,
    SIGNATURE_WINDOW_SECONDS, SIGNING_SCHEME, TIMESTAMP_HEADER, WIRE_HEADER, WIRE_VERSION,
    edge_key_from, edge_key_to, signing_input,
};
use crate::contract::{FNV1A64_OFFSET, FNV1A64_PRIME, contract_hash, service_normal_form};
use crate::resolver::cross_context_service_for;
use crate::wire::{BaseGuard, JsonKind, WIRE_DISCRIMINANT, field_base_guards, json_kind_of};

/// The context the page's examples and worked hash are computed on.
pub const EXAMPLE_SOURCE: &str = r#"context wire.sample

type Qty = Int where NonNegative

type Shape =
  | Circle(radius: Float)
  | Dot

type Line = {
  sku: String,
  qty: Int where Positive,
  wait: Duration,
  delay: Option[Duration],
  at: Instant,
  blob: Bytes,
  note: Option[String],
  tags: List[String],
  stock: Map[String, Int],
  rush: Bool,
}

service quote {
  on call(line: Line, size: Qty) -> Effect[Result[Shape, String]] {
    Ok(Dot)
  }
}

type Tagged[T] = {
  n: Int where Positive,
  v: T,
}

service who {
  on call(t: Tagged[String]) -> Effect[Result[String, String]] by c: Caller {
    Ok(c.identity)
  }
}

service ping {
  on call() -> Effect[Result[(), String]] {
    Ok(())
  }
}

actor Legacy { auth = Foreign(secret = "LEGACY_CALL_KEY") }

service legacy {
  on call(n: Int) -> Effect[Result[Int, String]] by Legacy {
    Ok(n)
  }
}
"#;

/// What decoding an example's JSON does.
pub enum Outcome {
    /// Decodes, and the decoded value encodes back as this JSON.
    Accepted { encodes_as: &'static str },
    /// Refused with this error `kind` at this `path`.
    Rejected {
        kind: &'static str,
        path: &'static str,
    },
}

/// One executed example: `json` decoded as `ty`.
pub struct Example {
    /// The Bynk type, as the page shows it.
    pub ty: &'static str,
    /// The generated codec suffix the test drives (`__deserialise_<codec>`).
    pub codec: &'static str,
    pub json: &'static str,
    pub outcome: Outcome,
    pub note: &'static str,
}

const LINE: &str = r#"{"sku":"A-1","qty":2,"wait":1500,"delay":{"kind":"Some","value":250},"at":1767225600000,"blob":"aGk=","note":{"kind":"Some","value":"gift"},"tags":["new"],"stock":[["lon",3]],"rush":false}"#;

/// The page's examples, each run through the generated codecs by
/// `bynkc/tests/wire_reference.rs`.
pub const EXAMPLES: &[Example] = &[
    Example {
        ty: "Line",
        codec: "Line",
        json: LINE,
        outcome: Outcome::Accepted { encodes_as: LINE },
        note: "Every field present. A record is an object keyed by field name.",
    },
    Example {
        ty: "Line",
        codec: "Line",
        json: r#"{"sku":"A-1","qty":2,"wait":1500,"at":1767225600000,"blob":"","tags":[],"stock":[],"rush":true,"extra":1}"#,
        outcome: Outcome::Accepted {
            encodes_as: r#"{"sku":"A-1","qty":2,"wait":1500,"delay":{"kind":"None"},"at":1767225600000,"blob":"","note":{"kind":"None"},"tags":[],"stock":[],"rush":true}"#,
        },
        note: "An absent `Option` field is `None`. An unknown key is ignored.",
    },
    Example {
        ty: "Line",
        codec: "Line",
        json: r#"{"sku":"A-1","qty":2,"wait":1500,"at":1767225600000,"blob":"","note":"gift","tags":[],"stock":[],"rush":true}"#,
        outcome: Outcome::Accepted {
            encodes_as: r#"{"sku":"A-1","qty":2,"wait":1500,"delay":{"kind":"None"},"at":1767225600000,"blob":"","note":{"kind":"Some","value":"gift"},"tags":[],"stock":[],"rush":true}"#,
        },
        note: "A bare value where an `Option` is expected is `Some`.",
    },
    Example {
        ty: "Line",
        codec: "Line",
        json: r#"{"sku":"A-1","qty":0,"wait":1500,"at":1767225600000,"blob":"","tags":[],"stock":[],"rush":true}"#,
        outcome: Outcome::Rejected {
            kind: "RefinementViolation",
            path: "$.qty",
        },
        note: "An inline field refinement (`qty: Int where Positive`) is checked.",
    },
    Example {
        ty: "Line",
        codec: "Line",
        json: r#"{"sku":"A-1","qty":1.5,"wait":1500,"at":1767225600000,"blob":"","tags":[],"stock":[],"rush":true}"#,
        outcome: Outcome::Rejected {
            kind: "StructuralMismatch",
            path: "$.qty",
        },
        note: "An `Int` is a whole number.",
    },
    Example {
        ty: "Line",
        codec: "Line",
        json: r#"{"sku":"A-1","qty":2,"wait":1.5,"at":1767225600000,"blob":"","tags":[],"stock":[],"rush":true}"#,
        outcome: Outcome::Rejected {
            kind: "StructuralMismatch",
            path: "$.wait",
        },
        note: "A `Duration` is whole milliseconds.",
    },
    Example {
        ty: "Line",
        codec: "Line",
        json: r#"{"sku":"A-1","qty":2,"wait":1500,"delay":{"kind":"Some","value":0.5},"at":1767225600000,"blob":"","tags":[],"stock":[],"rush":true}"#,
        outcome: Outcome::Rejected {
            kind: "StructuralMismatch",
            path: "$.delay.value",
        },
        note: "So is a `Duration` inside an `Option`, a `List` or a variant.",
    },
    Example {
        ty: "Line",
        codec: "Line",
        json: r#"{"sku":"A-1","qty":2,"wait":1500,"at":1767225600000,"blob":"%%","tags":[],"stock":[],"rush":true}"#,
        outcome: Outcome::Rejected {
            kind: "StructuralMismatch",
            path: "$.blob",
        },
        note: "`Bytes` is padded base64.",
    },
    Example {
        ty: "Line",
        codec: "Line",
        json: r#"{"sku":"A-1","qty":2,"wait":1500,"at":1767225600000,"blob":"","tags":[],"stock":{"lon":3},"rush":true}"#,
        outcome: Outcome::Rejected {
            kind: "StructuralMismatch",
            path: "$.stock",
        },
        note: "A `Map` is an array of pairs, never an object.",
    },
    Example {
        ty: "Qty",
        codec: "Qty",
        json: "-1",
        outcome: Outcome::Rejected {
            kind: "RefinementViolation",
            path: "$",
        },
        note: "A named refined type is its base value, checked against its predicates.",
    },
    Example {
        ty: "Tagged[String]",
        codec: "Tagged_String",
        json: r#"{"n":0,"v":"x"}"#,
        outcome: Outcome::Rejected {
            kind: "RefinementViolation",
            path: "$.n",
        },
        note: "A generic record's inline refinements are checked in every instantiation.",
    },
    Example {
        ty: "Shape",
        codec: "Shape",
        json: r#"{"kind":"Circle","radius":0.5}"#,
        outcome: Outcome::Accepted {
            encodes_as: r#"{"kind":"Circle","radius":0.5}"#,
        },
        note: "A variant's payload fields sit beside `kind`.",
    },
    Example {
        ty: "Shape",
        codec: "Shape",
        json: r#"{"kind":"Circle","radius":1e999}"#,
        outcome: Outcome::Rejected {
            kind: "StructuralMismatch",
            path: "$.radius",
        },
        note: "A `Float` is finite; `1e999` parses as infinity.",
    },
    Example {
        ty: "Shape",
        codec: "Shape",
        json: r#"{"kind":"Square"}"#,
        outcome: Outcome::Rejected {
            kind: "StructuralMismatch",
            path: "$",
        },
        note: "An unknown variant is refused.",
    },
    Example {
        ty: "Result[Shape, String]",
        codec: "Result_Shape_String",
        json: r#"{"kind":"Ok","value":{"kind":"Dot"}}"#,
        outcome: Outcome::Accepted {
            encodes_as: r#"{"kind":"Ok","value":{"kind":"Dot"}}"#,
        },
        note: "",
    },
    Example {
        ty: "Result[Shape, String]",
        codec: "Result_Shape_String",
        json: r#"{"kind":"Err","error":"sold out"}"#,
        outcome: Outcome::Accepted {
            encodes_as: r#"{"kind":"Err","error":"sold out"}"#,
        },
        note: "A domain error is a `200` carrying `Err`.",
    },
    Example {
        ty: "Result[(), String]",
        codec: "Result_Unit_String",
        json: r#"{"kind":"Ok","value":null}"#,
        outcome: Outcome::Accepted {
            encodes_as: r#"{"kind":"Ok","value":null}"#,
        },
        note: "`()` is `null`.",
    },
];

/// The page's worked contract-hash example: each `on call` service of
/// [`EXAMPLE_SOURCE`] with its normal form and hash, in source order.
pub fn example_contracts() -> Vec<(String, String, String)> {
    let tokens = bynk_syntax::lexer::tokenize(EXAMPLE_SOURCE).expect("the example source lexes");
    let unit = match bynk_syntax::parser::parse_unit(&tokens, EXAMPLE_SOURCE) {
        Ok(SourceUnit::Context(c)) => c,
        other => panic!("the example source is a context: {other:?}"),
    };
    let types: HashMap<String, Arc<TypeDecl>> = unit
        .items
        .iter()
        .filter_map(|i| match i {
            CommonsItem::Type(t) => Some((t.name.name.clone(), Arc::new(t.clone()))),
            _ => None,
        })
        .collect();
    unit.items
        .iter()
        .filter_map(|i| match i {
            CommonsItem::Service(s) => cross_context_service_for(&s.name.name, s),
            _ => None,
        })
        .map(|svc| {
            let form = service_normal_form(&svc, &types);
            let hash = contract_hash(&form);
            (svc.name, form, hash)
        })
        .collect()
}

fn json_kind_name(k: JsonKind) -> &'static str {
    match k {
        JsonKind::Number => "number",
        JsonKind::String => "string",
        JsonKind::Boolean => "boolean",
        JsonKind::Object => "object",
        JsonKind::Array => "array",
        JsonKind::Null => "null",
    }
}

/// The base-type rows of the encoding table, read off [`crate::wire`]'s model:
/// the JSON kind each base type wires as and the guards its decode applies.
fn base_type_rows(out: &mut String) {
    const BASES: [(BaseType, &str); 7] = [
        (BaseType::Int, "an integer"),
        (BaseType::Float, "a number"),
        (BaseType::Bool, "`true` or `false`"),
        (BaseType::String, "a string"),
        (
            BaseType::Bytes,
            "standard base64 with padding (RFC 4648 §4)",
        ),
        (BaseType::Duration, "whole milliseconds"),
        (BaseType::Instant, "milliseconds since the Unix epoch"),
    ];
    for (base, value) in BASES {
        let kind = json_kind_of(base);
        let rule = if base == BaseType::Bytes {
            "valid base64; anything else is `StructuralMismatch` (`expected: \"base64 string\"`)"
                .to_string()
        } else {
            let mut rule = format!("JSON {}", json_kind_name(kind));
            for guard in field_base_guards(base) {
                rule.push_str(match guard {
                    BaseGuard::Integral => {
                        ", and `Number.isSafeInteger` (whole, within ±(2^53 − 1)); otherwise `StructuralMismatch` (`expected: \"safe integer\"`)"
                    }
                    BaseGuard::Finite => {
                        ", and finite; otherwise `StructuralMismatch` (`expected: \"finite number\"`)"
                    }
                });
            }
            rule
        };
        let encode = match base {
            BaseType::Int | BaseType::Duration | BaseType::Instant => {
                "an unsafe value throws in the sender (a `500`), never a lossy number"
            }
            BaseType::Float => "a non-finite value throws in the sender",
            _ => "as is",
        };
        let _ = writeln!(
            out,
            "| `{}` | {} ({value}) | {rule} | {encode} |",
            base.name(),
            json_kind_name(kind),
        );
    }
}

fn status_rows(out: &mut String) {
    for (i, check) in CALL_CHECKS.iter().enumerate() {
        let response = check.response();
        let _ = writeln!(
            out,
            "| {} | {} | `{}` | {response} |",
            i + 1,
            check.condition(),
            check.status(),
        );
    }
}

/// The whole page, in mdBook form (`# H1` title, relative `.md` links).
pub fn render_reference() -> String {
    let mut out = String::new();
    let accepted = ACCEPTED_WIRE_VERSIONS
        .iter()
        .map(|v| format!("`{v}`"))
        .collect::<Vec<_>>()
        .join(", ");

    let from_example = edge_key_from("shop.orders").unwrap_or_default();
    let to_example = edge_key_to("shop.payment").unwrap_or_default();
    let input_example = signing_input(
        "shop.orders",
        "shop.payment",
        CALL_METHOD,
        &format!("{CALL_PATH_PREFIX}authorise"),
        "0123456789abcdef",
        "1",
        "1760000000",
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    );
    let _ = write!(
        out,
        r#"# Wire format (version {WIRE_VERSION})

<!-- GENERATED FILE — do not edit by hand.
     Source: bynk-check/src/wire_format.rs and wire_format/reference.rs, via bynkc/tests/wire_reference.rs.
     Regenerate with: BYNK_BLESS=1 cargo test -p bynkc --test it wire_reference:: -->

How one Bynk context calls another context's `on call` service over a
Cloudflare Service Binding: the request, the checks the callee makes, the
responses, the JSON encoding of every type that can cross, and the contract
hash. Code that is not Bynk can call a Bynk service by speaking this format.

This page is generated from the compiler. The header names, the check order
and the status codes are the constants the emitter builds the call dispatch
from; the encoding table is read from the compiler's wire model; and every
example below is run through the codecs the compiler generates for the
example context, by `bynkc/tests/wire_reference.rs`.

## Version and stability

This is wire format version **{WIRE_VERSION}**. It is versioned on its own, not
with the compiler: a compiler release changes it only by adding a new wire
version.

- **Within a version, changes are additive only.** A new status code, a new
  error `kind` or a new optional header may appear. A caller must treat an
  error `kind` it does not know as an error.
- **Anything else is a new version:** changing an encoding, a check, a status,
  an existing error body, or the contract hash's normal form or function.
- **The caller states its version** in `{WIRE_HEADER}`; an absent header means
  `1`. **The callee accepts a range** (this compiler's callees accept
  {accepted}) and refuses any other version with the versions it accepts, so a
  version is retired after an overlap window, not on a flag day.
- **The signature is part of the version:** its headers, its signing input
  and its window. The keys are not; they belong to a deployment.

What the format does **not** cover: the TypeScript the compiler emits, the
in-memory representation of values, and the Worker's other entry points
(`from http` routes, events, queues). Those follow the compiler's own version.

## Request

```text
{CALL_METHOD} {CALL_PATH_PREFIX}<service>
```

`<service>` is the service's name as declared (`service quote` is
`{CALL_PATH_PREFIX}quote`). The host is ignored: a Bynk caller sends
`http://internal{CALL_PATH_PREFIX}<service>` over the Service Binding.

| Header | Required | Meaning |
|---|---|---|
"#
    );
    for h in HEADERS {
        let _ = writeln!(out, "| `{}` | {} | {} |", h.name, h.required, h.meaning);
    }
    let _ = write!(
        out,
        r#"
**Body.** JSON, encoding the handler's parameters:

- **no parameters:** any JSON value; Bynk callers send `{{}}`;
- **one parameter:** the parameter's value itself, not wrapped;
- **two or more:** an object keyed by parameter name. Unknown keys are ignored.

So `on call(line: Line, size: Qty)` takes `{{"line": {{…}}, "size": 3}}`, and
`on call(size: Qty)` takes `3`. Renaming or reordering parameters is a contract
change (see [Contract hash](#contract-hash)).

## Checks and responses

The callee makes these checks in this order. The first that fails answers the
request.

| # | Check | Status on failure | Body on failure |
|---|---|---|---|
"#
    );
    status_rows(&mut out);
    let _ = write!(
        out,
        r#"
If every check passes, the handler runs:

- **`200`**, `content-type: application/json`, with the handler's return value
  encoded as below. A domain error is a `200` too: `Result[T, E]`'s `Err` is a
  value, not a status.
- **`500`**, text `Internal Server Error`, if the handler faults (including
  encoding a value the format cannot carry, such as an unsafe `Int`). The
  callee logs the fault; the body says nothing more. The response carries
  `X-Bynk-Fault: <context>`, the callee's qualified name, so the caller can
  say which Worker faulted. Only a `/_bynk/call/` fault is marked.

A status not listed here (a platform error between the Workers, for example)
is a transport failure. A Bynk caller reports a `409` `ContractMismatch` and a
marked `500` `CalleeFaulted` by name, and every other non-`2xx` as
`Transport`, with the status.

### Error bodies

Every JSON error body is an object with a `kind`:

| `kind` | Status | Fields |
|---|---|---|
| `UnsupportedWireVersion` | `400` | `accepted`: the wire versions the callee accepts |
| `ContractMismatch` | `409` | `service`, `expected` (the callee's hash), `actual` (the header as received) |
| `MalformedJson` | `400` | `details` |
| `StructuralMismatch` | `400` | `path`, `expected`, `actual`: the value had the wrong shape |
| `RefinementViolation` | `400` | `path`, `violation`: `{{"field", "message", "value"}}`, a value of the right shape broke a predicate |

`path` is a JSONPath-like location in the body: `$` is the whole body (one
parameter), `$.<param>` a parameter (several), then `.<field>` or `[<index>]`
inward. An unknown variant fails at the sum's own `path`, with
`expected: "sum variant kind"` and the `kind` that was sent as `actual`.

## Encoding

Each type has one JSON encoding, the same in both directions. A value is
decoded by checking its shape, then any predicates.

### Base types

| Type | JSON | Decode accepts | Encode |
|---|---|---|---|
"#
    );
    base_type_rows(&mut out);
    let _ = write!(
        out,
        r#"| `()` | `null` | anything; the value is not inspected | `null` |

`Int`, `Duration` and `Instant` are limited to JavaScript's safe-integer range
on the wire, so every JSON parser reads them exactly.

### Composite types

| Type | JSON |
|---|---|
| record | an object keyed by field name. A missing field decodes as its default when it has one, as `None` when it is an `Option`, and otherwise fails. Unknown keys are ignored. |
| sum (`enum` included) | an object with `"{WIRE_DISCRIMINANT}"`: the variant's name, beside the variant's payload fields: `{{"{WIRE_DISCRIMINANT}":"Circle","radius":0.5}}`, `{{"{WIRE_DISCRIMINANT}":"Dot"}}` |
| `Result[T, E]` | `{{"{WIRE_DISCRIMINANT}":"Ok","value":<T>}}` or `{{"{WIRE_DISCRIMINANT}":"Err","error":<E>}}` |
| `Option[T]` | `{{"{WIRE_DISCRIMINANT}":"Some","value":<T>}}` or `{{"{WIRE_DISCRIMINANT}":"None"}}`. A sender may also send `null` or leave the field out for `None`, or send the bare value for `Some`; Bynk always encodes the tagged form |
| `List[T]` | an array |
| `Map[K, V]` | an array of `[key, value]` pairs, in insertion order |
| refined type (`type Qty = Int where NonNegative`) | its base type's encoding. The decoder checks the predicates, refusing with `RefinementViolation` |
| opaque type | its base type's encoding. A caller cannot see the predicate, so only the owning context checks it |
| inline field refinement (`qty: Int where Positive`) | the field's encoding. The decoder checks the predicates, like a refined type's |
| generic instantiation (`Page[Line]`) | the declaration's encoding with the arguments substituted |

The in-memory discriminant of a Bynk sum is `tag`; on the wire it is
`"{WIRE_DISCRIMINANT}"`. The encoding never depends on the in-memory layout.

### Examples

Each row is decoded by the codec the compiler generates for `wire.sample`
below, then encoded again. Rejections show the error's `kind` and `path`.

```bynk
"#
    );
    out.push_str(EXAMPLE_SOURCE.trim_end());
    out.push_str("\n```\n\n| Type | JSON sent | Outcome | Note |\n|---|---|---|---|\n");
    for ex in EXAMPLES {
        let outcome = match &ex.outcome {
            Outcome::Accepted { encodes_as } if *encodes_as == ex.json => {
                "accepted; encodes back the same".to_string()
            }
            Outcome::Accepted { encodes_as } => {
                format!("accepted; encodes back as `{encodes_as}`")
            }
            Outcome::Rejected { kind, path } => format!("`{kind}` at `{path}`"),
        };
        let _ = writeln!(
            out,
            "| `{}` | `{}` | {outcome} | {} |",
            ex.ty, ex.json, ex.note
        );
    }

    let contracts = example_contracts();
    let _ = write!(
        out,
        r#"
## Contract hash

`X-Bynk-Contract` carries a hash of the service's signature, so a callee can
refuse a caller that was compiled against a different version of it (`409`).
It detects skew; it is not a credential and not a security control.

**Normal form.** One line per service:

```text
<service>(<param>: <type>, …) -> <type>
```

- Parameters keep their names and order. `Effect[T]` is written as `T`.
- Base types are their names; `()` is `()`; `List[T]`, `Option[T]`,
  `Map[K, V]`, `Result[T, E]` keep their arguments in order.
- A named type is its name followed by its structure, so both a rename and a
  change of shape change the hash:
  - a record is `Name{{f: T, …}}`, each field written `f: T` (or
    `f: T where P, …` with an inline refinement), the fields sorted by that
    text's UTF-8 bytes and joined with `, `;
  - a sum is `Name|A|B(T, …)`: each variant written as its name, followed by
    its payload types in order in parentheses if it has any, the variants
    sorted by that text's UTF-8 bytes, each preceded by `|`;
  - a refined type is its name and base run together, then its predicates:
    `type Qty = Int where NonNegative` is `QtyInt where NonNegative` (with no
    predicates, `QtyInt ` with a trailing space);
  - an opaque type is `NameBase opaque`, without its predicates;
  - a generic instantiation is the declaration's form with the arguments
    substituted, followed by `[A, …]`;
  - a type met again inside its own expansion is `@Name`.
- Predicates are a set: written as below, sorted by their UTF-8 bytes, with
  duplicates removed, and joined with `, `. They are `Positive`,
  `NonNegative`, `InRange(a, b)`, `InRangeF(a, b)`, `MinLength(n)`,
  `MaxLength(n)`, `Length(n)` and `Matches("…")`; `NonEmpty` is written
  `MinLength(1)`. Bounds are written as decimal numbers in their shortest
  round-trip form (`01` is `1`). A `Matches` pattern is quoted, with `\`, `"`
  and control characters backslash-escaped.
- No spaces other than those shown; no doc comments, defaults, `given` or
  `by` clauses.

**Hash.** FNV-1a, 64-bit, over the normal form's UTF-8 bytes: start from the
offset basis `0x{FNV1A64_OFFSET:016x}`; for each byte, XOR it in, then
multiply by the prime `0x{FNV1A64_PRIME:016x}` modulo 2^64. The result is
written as 16 lowercase hex digits.

For `wire.sample` above, each service's normal form, byte for byte, and its
hash:
"#
    );
    for (name, form, hash) in &contracts {
        let _ = write!(out, "\n`{name}`, hash `{hash}`:\n\n```text\n{form}\n```\n");
    }
    let _ = write!(
        out,
        r#"
| Changes the hash | Does not |
|---|---|
| the service's name; a parameter's name, order or type; the return type | field order; variant order; predicate order or duplicates |
| a field added, removed, renamed or retyped; a variant added, removed, renamed or re-shaped | an opaque type's predicates |
| a refined type's or an inline field refinement's predicates | doc comments; `given`; `by c: Caller` |
| renaming a named type | field defaults |

The normal form and the hash function are part of the wire version: a client
built once embeds hashes, so they change only with a new wire version.

## Authentication

Every call is signed. The callee refuses a call whose signature does not verify
with a bare `401`, before it looks at anything but the method and the body's
size, so a sender without a key learns nothing: not which services exist, not
a contract hash.

**Keys.** One key per *edge*, a caller and a callee. The callee holds it as
`{EDGE_FROM_PREFIX}<CALLER>` and the caller as `{EDGE_TO_PREFIX}<CALLEE>`, where
`<CALLER>` and `<CALLEE>` are the qualified context names upper-cased with each
dot written `__` (`shop.orders` → `{from_example}`, `shop.payment` →
`{to_example}`). A name the key can be named from is dot-separated segments of
lowercase letters and digits, each starting with a letter, with single
underscores between runs. `bynk deploy` mints each key and sets it on both
Workers; `bynk dev` uses a fixed local key.

**Signing.** The caller sends `{CALLER_HEADER}` (its qualified name),
`{TIMESTAMP_HEADER}` (whole seconds since the Unix epoch) and `{SIGNATURE_HEADER}`:
HMAC-SHA256, under the edge's key, of the *signing input*, written as 64
lowercase hex digits. The signing input is these fields, one per line,
separated by `\n` with no trailing newline:

1. `{SIGNING_SCHEME}`
2. the caller, as sent in `{CALLER_HEADER}`
3. the callee's qualified context name
4. the method
5. the URL's pathname exactly as sent, with no query string and no
   percent-normalisation
6. `{CONTRACT_HEADER}` as sent, empty when absent
7. `{WIRE_HEADER}` as sent, `1` when absent
8. `{TIMESTAMP_HEADER}` as sent
9. the SHA-256 of the body's bytes, 64 lowercase hex digits

For example, `shop.orders` calling `authorise` on `shop.payment` with an empty
body signs:

```text
{input_example}
```

**Verifying.** The callee reads the body under its cap (1 MiB, {MAX_BODY_BYTES}
bytes, else `413`), picks the key it holds for the edge from `{CALLER_HEADER}`,
and refuses (`401`) when it holds none, when `{TIMESTAMP_HEADER}` is more than
{SIGNATURE_WINDOW_SECONDS} seconds from its clock either way, or when the
signature does not match, compared in constant time. So:

- **A key's holder can claim only its own name.** A caller's name is the
  handler's `c.identity`, proved by the key, not claimed.
- **A key binds one callee.** The callee's name is signed, so a request cannot
  be replayed against another callee, even one that holds the same key.
- **Every header the callee acts on is signed**, so none can be changed in
  flight: not the contract, not the wire version.
- **A captured call can be replayed inside the window.** The timestamp bounds
  replay; nothing stops it within those {SIGNATURE_WINDOW_SECONDS} seconds. An
  `on call` handler with side effects that must happen once should make itself
  idempotent (the `Idempotency` capability).
- **An undeclared caller cannot call.** Without a key on the callee there is
  nothing to verify against, whether or not the handler reads its caller.

### Foreign callers

Code that is not Bynk calls a context the same way, once the context declares
it as a **foreign caller**:

```text
actor Legacy {{ auth = Foreign(secret = "LEGACY_CALL_KEY") }}

service legacy {{
  on call(n: Int) -> Effect[Result[Int, String]] by Legacy {{ Ok(n) }}
}}
```

The foreign caller sends its actor's name (`Legacy`) as `{CALLER_HEADER}` and
signs exactly as above, under the declared secret, which the callee holds as
`LEGACY_CALL_KEY`. `bynk deploy` mints that secret and prints the
`wrangler secret put` that hands it to the foreign Worker's operator. An
actor's name starts with a capital letter, so it is never a context's: a
foreign caller is checked under its own secret and can never pass for a Bynk
context, nor a context for it.

A verified caller still has to be one the service's handler admits. A Bynk
context is admitted by `Caller` (the default, with no `by`), a foreign caller
only by its own actor; `by who: Caller | Legacy` admits both and tells them
apart. Anyone else gets a bare `403`.

A TypeScript Worker need not speak any of this by hand. For each context with
a foreign caller the build emits a typed client, `bynk-client.ts`, which
signs, stamps the contract hash and encodes exactly as this page describes;
see [TypeScript client](/book/reference/typescript-client/).
"#
    );
    out
}
