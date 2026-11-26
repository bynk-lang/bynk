//! #4099: the JSON Schema of a type, for `Ai.generateValue[T]`.
//!
//! The compiler writes `T`'s schema into the request, so the model is asked for the shape the reply
//! is then decoded as. It is the shape a plain-JSON route writes (#3974): an `enum` is a string, an
//! `Option` is the value or `null`, a `String`-keyed `Map` is an object and an `Instant` is RFC 3339
//! text; a sum with payloads is an object with `"kind"` beside the payload fields. The ordinary
//! decoder reads all of those, and checks the refinements the schema can only state.
//!
//! The schema is a string of compact JSON in a fixed key order, so it is the same on every build.
//! A type with no schema (a recursive one, a generic application) is an error naming the field.

use std::collections::HashMap;
use std::sync::Arc;

use bynk_syntax::ast::{
    BaseType, PredBound, PredKind, RecordField, Refinement, TypeBody, TypeDecl, TypeRef,
};

/// Why a type has no schema, and where in it: `path` is the type then the fields down to it
/// (`Triage.tags`), as the diagnostic names them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaError {
    pub path: String,
    pub reason: String,
}

/// A JSON value, written in the order it was built.
enum J {
    Obj(Vec<(&'static str, J)>),
    Named(Vec<(String, J)>),
    Arr(Vec<J>),
    Str(String),
    Num(f64),
    Bool(bool),
}

impl J {
    fn s(text: &str) -> J {
        J::Str(text.to_string())
    }

    fn render(&self, out: &mut String) {
        match self {
            J::Obj(fields) => {
                out.push('{');
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    quote(k, out);
                    out.push(':');
                    v.render(out);
                }
                out.push('}');
            }
            J::Named(fields) => {
                out.push('{');
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    quote(k, out);
                    out.push(':');
                    v.render(out);
                }
                out.push('}');
            }
            J::Arr(items) => {
                out.push('[');
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    v.render(out);
                }
                out.push(']');
            }
            J::Str(s) => quote(s, out),
            J::Num(n) => {
                if n.fract() == 0.0 && n.abs() < 9.0e15 {
                    out.push_str(&format!("{}", *n as i64));
                } else {
                    out.push_str(&format!("{n}"));
                }
            }
            J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        }
    }
}

fn quote(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

type Types = HashMap<String, Arc<TypeDecl>>;

/// The schema of `t` as compact JSON, or why it has none. `root` names the type in a message.
pub fn schema_of(t: &TypeRef, root: &str, types: &Types) -> Result<String, SchemaError> {
    let mut stack = Vec::new();
    let j = type_schema(t, root, types, &mut stack)?;
    let mut out = String::new();
    j.render(&mut out);
    Ok(out)
}

fn fail<T>(path: &str, reason: impl Into<String>) -> Result<T, SchemaError> {
    Err(SchemaError {
        path: path.to_string(),
        reason: reason.into(),
    })
}

fn type_schema(
    t: &TypeRef,
    path: &str,
    types: &Types,
    stack: &mut Vec<String>,
) -> Result<J, SchemaError> {
    match t {
        TypeRef::Base(b, _) => Ok(base_schema(*b, None)),
        TypeRef::Option(inner, _) => Ok(J::Obj(vec![(
            "anyOf",
            J::Arr(vec![
                type_schema(inner, path, types, stack)?,
                J::Obj(vec![("type", J::s("null"))]),
            ]),
        )])),
        TypeRef::List(inner, _) => Ok(J::Obj(vec![
            ("type", J::s("array")),
            (
                "items",
                type_schema(inner, &format!("{path}[]"), types, stack)?,
            ),
        ])),
        TypeRef::Set(inner, _) => Ok(J::Obj(vec![
            ("type", J::s("array")),
            ("uniqueItems", J::Bool(true)),
            (
                "items",
                type_schema(inner, &format!("{path}[]"), types, stack)?,
            ),
        ])),
        TypeRef::Map(key, value, _) => {
            if !is_string_key(key, types) {
                return fail(
                    path,
                    "a `Map` is an object in JSON only when its key is a `String`",
                );
            }
            Ok(J::Obj(vec![
                ("type", J::s("object")),
                (
                    "additionalProperties",
                    type_schema(value, &format!("{path}{{}}"), types, stack)?,
                ),
            ]))
        }
        TypeRef::Result(ok, err, _) => Ok(J::Obj(vec![(
            "oneOf",
            J::Arr(vec![
                kind_object(
                    "Ok",
                    vec![("value".to_string(), type_schema(ok, path, types, stack)?)],
                ),
                kind_object(
                    "Err",
                    vec![("error".to_string(), type_schema(err, path, types, stack)?)],
                ),
            ]),
        )])),
        TypeRef::Named(id) => named_schema(&id.name, path, types, stack),
        TypeRef::Unit(_) => fail(path, "`()` has no value to ask for"),
        TypeRef::App { .. } => fail(
            path,
            "a generic type has no schema; use a type that is not generic",
        ),
        _ => fail(path, "this type has no JSON form"),
    }
}

fn is_string_key(key: &TypeRef, types: &Types) -> bool {
    match key {
        TypeRef::Base(BaseType::String, _) => true,
        TypeRef::Named(id) => types.get(&id.name).is_some_and(|d| {
            matches!(
                &d.body,
                TypeBody::Refined {
                    base: BaseType::String,
                    ..
                } | TypeBody::Opaque {
                    base: BaseType::String,
                    ..
                }
            )
        }),
        _ => false,
    }
}

fn named_schema(
    name: &str,
    path: &str,
    types: &Types,
    stack: &mut Vec<String>,
) -> Result<J, SchemaError> {
    let Some(decl) = types.get(name) else {
        return fail(path, format!("`{name}` is not a type this unit can see"));
    };
    if stack.iter().any(|n| n == name) {
        return fail(
            path,
            format!("`{name}` contains itself, and a schema cannot say so"),
        );
    }
    if !decl.type_params.is_empty() {
        return fail(
            path,
            format!("`{name}` is generic, and a generic type has no schema"),
        );
    }
    stack.push(name.to_string());
    let out = match &decl.body {
        TypeBody::Refined {
            base, refinement, ..
        }
        | TypeBody::Opaque {
            base, refinement, ..
        } => Ok(base_schema(*base, refinement.as_ref())),
        TypeBody::Record(record) => {
            let mut properties = Vec::new();
            let mut required = Vec::new();
            for f in &record.fields {
                let key = f.wire_name().unwrap_or_else(|| f.name.name.clone());
                let at = format!("{name}.{}", f.name.name);
                let j = field_schema(f, &at, types, stack)?;
                // An `Option` field is required and nullable, as the published schema of a
                // plain-JSON route has it (the plain writer always writes the key).
                required.push(J::Str(key.clone()));
                properties.push((key, j));
            }
            Ok(J::Obj(vec![
                ("type", J::s("object")),
                ("properties", J::Named(properties)),
                ("required", J::Arr(required)),
                ("additionalProperties", J::Bool(false)),
            ]))
        }
        TypeBody::Sum(sum) => {
            if sum.variants.iter().all(|v| v.payload.is_empty()) {
                Ok(J::Obj(vec![
                    ("type", J::s("string")),
                    (
                        "enum",
                        J::Arr(
                            sum.variants
                                .iter()
                                .map(|v| J::Str(v.name.name.clone()))
                                .collect(),
                        ),
                    ),
                ]))
            } else {
                let mut variants = Vec::new();
                for v in &sum.variants {
                    let mut fields = Vec::new();
                    for p in &v.payload {
                        let at = format!("{name}.{}.{}", v.name.name, p.name.name);
                        fields.push((
                            p.name.name.clone(),
                            type_schema(&p.type_ref, &at, types, stack)?,
                        ));
                    }
                    variants.push(kind_object(&v.name.name, fields));
                }
                Ok(J::Obj(vec![("oneOf", J::Arr(variants))]))
            }
        }
    };
    stack.pop();
    out
}

/// A field's schema: its type's, with the field's own refinement applied.
fn field_schema(
    f: &RecordField,
    path: &str,
    types: &Types,
    stack: &mut Vec<String>,
) -> Result<J, SchemaError> {
    if let (TypeRef::Base(b, _), Some(r)) = (&f.type_ref, &f.refinement) {
        return Ok(base_schema(*b, Some(r)));
    }
    type_schema(&f.type_ref, path, types, stack)
}

/// `{"kind": "<name>", <fields…>}`, every field required.
fn kind_object(name: &str, fields: Vec<(String, J)>) -> J {
    let mut properties = vec![("kind".to_string(), J::Obj(vec![("const", J::s(name))]))];
    let mut required = vec![J::s("kind")];
    for (k, v) in fields {
        required.push(J::Str(k.clone()));
        properties.push((k, v));
    }
    J::Obj(vec![
        ("type", J::s("object")),
        ("properties", J::Named(properties)),
        ("required", J::Arr(required)),
    ])
}

fn base_schema(b: BaseType, refinement: Option<&Refinement>) -> J {
    let mut fields: Vec<(&'static str, J)> = match b {
        BaseType::Int => vec![("type", J::s("integer"))],
        BaseType::Float => vec![("type", J::s("number"))],
        BaseType::Bool => vec![("type", J::s("boolean"))],
        BaseType::String => vec![("type", J::s("string"))],
        BaseType::Bytes => vec![
            ("type", J::s("string")),
            ("contentEncoding", J::s("base64")),
        ],
        // Plain JSON writes an `Instant` as RFC 3339 text; the decoder reads it.
        BaseType::Instant => vec![("type", J::s("string")), ("format", J::s("date-time"))],
        BaseType::Decimal => vec![
            ("type", J::s("string")),
            ("pattern", J::s("^-?[0-9]+(\\.[0-9]+)?$")),
        ],
        BaseType::Duration => vec![("type", J::s("integer"))],
    };
    if let Some(r) = refinement {
        let mut bounds = Bounds::default();
        for p in &r.predicates {
            bounds.add(&p.kind);
        }
        bounds.write(b, &mut fields);
    }
    J::Obj(fields)
}

/// The predicates a schema can state; the rest the decoder still checks.
#[derive(Default)]
struct Bounds {
    min_length: Option<f64>,
    max_length: Option<f64>,
    minimum: Option<f64>,
    maximum: Option<f64>,
    exclusive_minimum: Option<f64>,
}

impl Bounds {
    fn add(&mut self, p: &PredKind) {
        let lower = |slot: &mut Option<f64>, n: f64| {
            *slot = Some(slot.map_or(n, |old| old.max(n)));
        };
        let upper = |slot: &mut Option<f64>, n: f64| {
            *slot = Some(slot.map_or(n, |old| old.min(n)));
        };
        let number = |b: &PredBound| match b {
            PredBound::Int(a) => Some(a.value as f64),
            PredBound::Float(a) => Some(a.value),
            PredBound::Duration(a) => Some(a.millis as f64),
            PredBound::Decimal(_) => None,
        };
        match p {
            PredKind::MinLength(n) => lower(&mut self.min_length, *n as f64),
            PredKind::MaxLength(n) => upper(&mut self.max_length, *n as f64),
            PredKind::Length(n) => {
                lower(&mut self.min_length, *n as f64);
                upper(&mut self.max_length, *n as f64);
            }
            PredKind::NonEmpty => lower(&mut self.min_length, 1.0),
            PredKind::NonNegative => lower(&mut self.minimum, 0.0),
            PredKind::Positive => lower(&mut self.exclusive_minimum, 0.0),
            PredKind::InRange(a, b) => {
                lower(&mut self.minimum, a.value as f64);
                upper(&mut self.maximum, b.value as f64);
            }
            PredKind::InRangeF(a, b) => {
                lower(&mut self.minimum, a.value);
                upper(&mut self.maximum, b.value);
            }
            PredKind::InRangeDur(a, b) => {
                lower(&mut self.minimum, a.millis as f64);
                upper(&mut self.maximum, b.millis as f64);
            }
            PredKind::AtLeast(b) => {
                if let Some(n) = number(b) {
                    lower(&mut self.minimum, n);
                }
            }
            PredKind::AtMost(b) => {
                if let Some(n) = number(b) {
                    upper(&mut self.maximum, n);
                }
            }
            // A pattern, a date or a cron schedule, and a decimal's range, are checked by the
            // decoder; the schema does not repeat them.
            PredKind::Matches(_) | PredKind::InRangeD(..) | PredKind::IsoDate | PredKind::Cron => {}
        }
    }

    fn write(&self, base: BaseType, fields: &mut Vec<(&'static str, J)>) {
        let text = matches!(base, BaseType::String);
        let number = matches!(base, BaseType::Int | BaseType::Float | BaseType::Duration);
        if text {
            if let Some(n) = self.min_length {
                fields.push(("minLength", J::Num(n)));
            }
            if let Some(n) = self.max_length {
                fields.push(("maxLength", J::Num(n)));
            }
        }
        if number {
            if let Some(n) = self.minimum {
                fields.push(("minimum", J::Num(n)));
            }
            if let Some(n) = self.maximum {
                fields.push(("maximum", J::Num(n)));
            }
            if let Some(n) = self.exclusive_minimum {
                fields.push(("exclusiveMinimum", J::Num(n)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn types_of(src: &str) -> Types {
        let source = format!("commons demo\n\n{src}\n");
        let tokens = bynk_syntax::lexer::tokenize(&source).expect("lexes");
        let (unit, errors) = bynk_syntax::parser::parse_unit_with_recovery(&tokens, &source);
        assert!(errors.is_empty(), "{errors:?}");
        let Some(bynk_syntax::ast::SourceUnit::Commons(c)) = unit else {
            panic!("a commons");
        };
        c.items
            .iter()
            .filter_map(|i| match i {
                bynk_syntax::ast::CommonsItem::Type(t) => {
                    Some((t.name.name.clone(), Arc::new(t.clone())))
                }
                _ => None,
            })
            .collect()
    }

    fn schema(src: &str, root: &str) -> Result<String, SchemaError> {
        let types = types_of(src);
        let t = TypeRef::Named(bynk_syntax::ast::Ident {
            name: root.to_string(),
            span: bynk_syntax::span::Span::default(),
        });
        schema_of(&t, root, &types)
    }

    const TRIAGE: &str = "type Severity = enum { Low, Medium, High }\n\
        type Summary = String where MaxLength(200)\n\
        type Triage = { severity: Severity, summary: Summary, tags: List[String], note: Option[String], score: Int where InRange(0, 10) }";

    #[test]
    fn a_record_is_an_object_with_its_fields_and_refinements() {
        let s = schema(TRIAGE, "Triage").expect("a schema");
        assert!(
            s.starts_with("{\"type\":\"object\",\"properties\":{\"severity\":"),
            "{s}"
        );
        assert!(s.contains("\"enum\":[\"Low\",\"Medium\",\"High\"]"), "{s}");
        assert!(s.contains("\"maxLength\":200"), "{s}");
        assert!(s.contains("\"minimum\":0,\"maximum\":10"), "{s}");
        // `note` is an `Option`: nullable, and still required, as the plain writer has it.
        assert!(
            s.contains("\"note\":{\"anyOf\":[{\"type\":\"string\"},{\"type\":\"null\"}]}"),
            "{s}"
        );
        assert!(
            s.ends_with("\"required\":[\"severity\",\"summary\",\"tags\",\"note\",\"score\"],\"additionalProperties\":false}"),
            "{s}"
        );
    }

    #[test]
    fn a_sum_with_payloads_is_a_discriminated_one_of() {
        let s =
            schema("type Shape =\n  | Circle(radius: Float)\n  | Dot", "Shape").expect("a schema");
        assert!(
            s.starts_with(
                "{\"oneOf\":[{\"type\":\"object\",\"properties\":{\"kind\":{\"const\":\"Circle\"}"
            ),
            "{s}"
        );
        assert!(s.contains("\"kind\":{\"const\":\"Dot\"}"), "{s}");
    }

    #[test]
    fn a_recursive_type_has_no_schema_and_the_error_names_the_field() {
        let err = schema(
            "type Tree = { label: String, children: List[Tree] }",
            "Tree",
        )
        .expect_err("recursive");
        assert_eq!(err.path, "Tree.children[]");
        assert!(err.reason.contains("contains itself"), "{err:?}");
    }

    #[test]
    fn the_schema_is_the_same_every_time() {
        assert_eq!(schema(TRIAGE, "Triage"), schema(TRIAGE, "Triage"));
    }
}
