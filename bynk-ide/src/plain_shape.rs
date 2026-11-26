//! #4453: the wire shape a record field is written in on a plain-JSON route.
//!
//! A route with `json { options: plain }` (or `@json(options: plain)`) writes an `enum` as a bare string,
//! a `String`-keyed `Map` as an object and an `Instant` as RFC 3339 text (#3974). The Bynk type on a
//! field's hover does not say so, so a field of a type some plain route writes gets one more section: the
//! routes that write it plainly and the shape it takes there. A tagged route changes nothing, and so does a
//! field whose type has no plain-only shape.
//!
//! "A type a route writes" is the route's response type and every type it reaches through record fields,
//! variant payloads, a generic application's declaration and arguments, and the `Option`, `List`, `Map`, `Set` and `Result` it is wrapped in. The reach is by type
//! name in the route's own type table (the table the wire peek reads), so a name two units both declare is read
//! as the same type: the section can name a route that writes a same-named type, never miss one that writes this.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use bynk_syntax::ast::{
    BaseType, HandlerKind, JsonOptions, TypeBody, TypeDecl, TypeRef, effective_json_options,
};

use crate::ContextBoundaryInfo;

/// The most plain routes a section names; the rest are counted.
const ROUTES_NAMED: usize = 3;

/// The section to append to the hover of the record field `type_name.field`, or `None` when no plain route
/// writes the type or the field has no plain-only shape.
pub(crate) fn field_section(
    type_name: &str,
    field: &str,
    infos: &HashMap<String, ContextBoundaryInfo>,
) -> Option<String> {
    let mut routes: BTreeSet<String> = BTreeSet::new();
    let mut shape: Option<String> = None;
    for info in infos.values() {
        let Some(decl) = info.types.get(type_name) else {
            continue;
        };
        let TypeBody::Record(record) = &decl.body else {
            continue;
        };
        let Some(f) = record.fields.iter().find(|f| f.name.name == field) else {
            continue;
        };
        let Some(described) = shape_of(&f.type_ref, &info.types) else {
            continue;
        };
        for service in info.services.values() {
            for handler in &service.handlers {
                let HandlerKind::Http { method, path } = &handler.kind else {
                    continue;
                };
                if effective_json_options(service.json.as_ref(), handler) != JsonOptions::Plain {
                    continue;
                }
                let mut reached = BTreeSet::new();
                if let Some(t) = response_type(&handler.return_type) {
                    reach(t, &info.types, &mut reached);
                }
                if reached.contains(type_name) {
                    routes.insert(format!("{} {path}", method.as_str()));
                    shape.get_or_insert(described.clone());
                }
            }
        }
    }
    let shape = shape?;
    let named: Vec<String> = routes
        .iter()
        .take(ROUTES_NAMED)
        .map(|r| format!("`{r}`"))
        .collect();
    let more = routes.len().saturating_sub(ROUTES_NAMED);
    let on = if more > 0 {
        format!("{} and {more} more", named.join(", "))
    } else {
        named.join(", ")
    };
    Some(format!(
        "\n\n**On a plain-JSON route** ({on}): {shape}. A route that writes the tagged form keeps the type's own form."
    ))
}

/// The type a route answers with: `Effect[HttpResult[T]]` and `HttpResult[T]` give `T`.
fn response_type(t: &TypeRef) -> Option<&TypeRef> {
    match t {
        TypeRef::Effect(inner, _) => response_type(inner),
        TypeRef::HttpResult(inner, _, _) => Some(inner),
        _ => None,
    }
}

/// The names of the types `t` reaches, itself included, through the types in `types`.
fn reach(t: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>, out: &mut BTreeSet<String>) {
    match t {
        TypeRef::Named(id) => {
            if !out.insert(id.name.clone()) {
                return;
            }
            let Some(decl) = types.get(&id.name) else {
                return;
            };
            match &decl.body {
                TypeBody::Record(r) => r.fields.iter().for_each(|f| reach(&f.type_ref, types, out)),
                TypeBody::Sum(s) => s
                    .variants
                    .iter()
                    .flat_map(|v| &v.payload)
                    .for_each(|p| reach(&p.type_ref, types, out)),
                TypeBody::Refined { .. } | TypeBody::Opaque { .. } => {}
            }
        }
        TypeRef::App { name, args, .. } => {
            reach(&TypeRef::Named(name.clone()), types, out);
            args.iter().for_each(|a| reach(a, types, out));
        }
        TypeRef::Option(inner, _)
        | TypeRef::List(inner, _)
        | TypeRef::Set(inner, _)
        | TypeRef::Effect(inner, _) => reach(inner, types, out),
        TypeRef::Result(a, b, _) | TypeRef::Map(a, b, _) => {
            reach(a, types, out);
            reach(b, types, out);
        }
        _ => {}
    }
}

/// How a field of type `t` is written on a plain route, when that differs from the tagged form: an `enum`,
/// a `String`-keyed `Map` and an `Instant`, alone or inside an `Option` or `List`.
fn shape_of(t: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>) -> Option<String> {
    match t {
        TypeRef::Base(BaseType::Instant, _) => {
            Some("RFC 3339 text in UTC, such as `\"2026-11-22T10:00:00.000Z\"`".into())
        }
        TypeRef::Map(key, _, _) if string_like(key, types) => {
            Some("an object keyed by the map key".into())
        }
        TypeRef::Named(id) => match types.get(&id.name).map(|d| &d.body) {
            Some(TypeBody::Sum(s)) if s.variants.iter().all(|v| v.payload.is_empty()) => {
                Some("a bare string, the variant's name".into())
            }
            _ => None,
        },
        TypeRef::Option(inner, _) => {
            shape_of(inner, types).map(|s| format!("{s}, or `null` for `None`"))
        }
        TypeRef::List(inner, _) => shape_of(inner, types).map(|s| format!("an array of: {s}")),
        _ => None,
    }
}

/// Whether `t` is a `String`, or a refined or opaque `String`: the keys a plain-JSON `Map` writes as an object.
fn string_like(t: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>) -> bool {
    match t {
        TypeRef::Base(BaseType::String, _) => true,
        TypeRef::Named(id) => matches!(
            types.get(&id.name).map(|d| &d.body),
            Some(
                TypeBody::Refined {
                    base: BaseType::String,
                    ..
                } | TypeBody::Opaque {
                    base: BaseType::String,
                    ..
                }
            )
        ),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    /// One context with a `plain` route and a `tagged` route, each answering with its own record of the same
    /// three fields: an `enum`, a `String`-keyed `Map` and an `Instant`.
    const SRC: &str = "context shop.users

type Role = enum { Admin, Guest }

type PlainUser = {
  role: Role,
  tags: Map[String, Int],
  joined: Instant,
  note: String,
}

type TaggedUser = {
  role: Role,
  tags: Map[String, Int],
  joined: Instant,
}

service plain from http {
  json { options: plain }
  on GET(\"/plain\") () -> Effect[HttpResult[PlainUser]] by v: Visitor {
    Ok(PlainUser { role: Admin, tags: Map.empty().insert(\"k\", 1), joined: Instant.fromEpochMillis(0), note: \"n\" })
  }
}

service tagged from http {
  on GET(\"/tagged\") () -> Effect[HttpResult[TaggedUser]] by v: Visitor {
    Ok(TaggedUser { role: Admin, tags: Map.empty().insert(\"k\", 1), joined: Instant.fromEpochMillis(0) })
  }
}
";

    /// Removes the temp project when the call that made it ends.
    struct Cleanup(PathBuf);

    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The hover over the `nth` `needle` of [`SRC`], through the real ladder.
    fn hover(needle: &str, nth: usize) -> Option<String> {
        hover_in(SRC, needle, nth)
    }

    /// The hover over the `nth` `needle` of the project's one file `src`, through the real ladder.
    fn hover_in(src: &str, needle: &str, nth: usize) -> Option<String> {
        // One directory per call: the tests run in parallel.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("bynk-plain-shape-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _cleanup = Cleanup(root.clone());
        let file = root.join("shop/users.bynk");
        std::fs::create_dir_all(file.parent().unwrap()).expect("create dirs");
        std::fs::write(&file, src).expect("write fixture");
        let root = root.canonicalize().unwrap_or(root);
        let r = crate::testkit::diagnose_project(&root);
        for f in &r.files {
            assert!(
                f.diagnostics.is_empty(),
                "the fixture analyses cleanly: {:?}",
                f.diagnostics
            );
        }
        let key = |p: &Path| -> PathBuf {
            r.index
                .symbols
                .values()
                .filter_map(|e| e.def.as_ref())
                .map(|d| d.path.clone())
                .find(|k| p.ends_with(k) || k.ends_with(p) || k.file_name() == p.file_name())
                .unwrap_or_else(|| p.to_path_buf())
        };
        let snapshots: HashMap<PathBuf, String> = r
            .files
            .iter()
            .map(|f| (key(&f.source_path), f.text.clone()))
            .collect();
        let rel = key(&r.files[0].source_path);
        let text = &r.files[0].text;
        let offset = text.match_indices(needle).nth(nth).expect("occurrence").0;
        crate::hover::hover_content(&crate::hover::HoverInput {
            analysis: Some(crate::hover::HoverAnalysis {
                index: &r.index,
                snapshots: &snapshots,
                locals: &r.locals,
                expr_types: &r.expr_types,
                tys: &r.ty_intern,
                rel: &rel,
                offset,
                capability_paths: &r.capability_paths,
                project_root: &root,
                doc_scope: &r.doc_scope,
                boundary_info: &r.boundary_info,
                context_count: crate::wire_contract::real_context_count(
                    &r.boundary_info,
                    &r.unit_sources,
                ),
            }),
            doc: Some((text, offset)),
            path: None,
            files: None,
        })
    }

    #[test]
    fn an_enum_field_of_a_plain_route_hovers_as_a_bare_string() {
        let h = hover("role: Role", 0).expect("the field hovers");
        assert!(
            h.contains("**On a plain-JSON route** (`GET /plain`)"),
            "{h}"
        );
        assert!(h.contains("a bare string, the variant's name"), "{h}");
    }

    #[test]
    fn a_string_keyed_map_field_of_a_plain_route_hovers_as_an_object() {
        let h = hover("tags: Map", 0).expect("the field hovers");
        assert!(h.contains("an object keyed by the map key"), "{h}");
    }

    #[test]
    fn an_instant_field_of_a_plain_route_hovers_as_rfc_3339_text() {
        let h = hover("joined: Instant", 0).expect("the field hovers");
        assert!(h.contains("RFC 3339 text in UTC"), "{h}");
    }

    #[test]
    fn a_field_with_no_plain_only_shape_is_unchanged() {
        let h = hover("note: String", 0).expect("the field hovers");
        assert!(!h.contains("plain-JSON route"), "{h}");
    }

    #[test]
    fn the_same_fields_of_a_tagged_route_are_unchanged() {
        // The second declaration of each field is `TaggedUser`'s: the type only the tagged route answers with.
        for (needle, nth) in [("role: Role", 1), ("tags: Map", 1), ("joined: Instant", 1)] {
            let h = hover(needle, nth).expect("the field hovers");
            assert!(!h.contains("plain-JSON route"), "{needle}: {h}");
        }
    }

    /// A route's own `@json` wins over its service's `json { }`: one record each way, and a type that
    /// is reached only through other types (an `Option`, a `List`, a `Set`, a generic application and a
    /// variant payload) and so is not any route's response type itself.
    const REACH: &str = "context shop.users

type Role = enum { Admin, Guest }

type Inner = { role: Role, at: Option[Instant], roles: List[Role] }
type Item = { role: Role }
type Member = { role: Role }
type Payload = { role: Role }
type Page[T] = { items: List[T] }
type Choice = | Pick(payload: Payload) | Skip
type Envelope = { inner: Option[Inner], page: Page[Item], members: Set[Member], choice: Choice }

type Forced = { role: Role }
type Kept = { role: Role }

service outer from http {
  json { options: plain }
  on GET(\"/envelope\") () -> Effect[HttpResult[Envelope]] by v: Visitor {
    Ok(Envelope { inner: None, page: Page { items: [] }, members: Set.empty(), choice: Skip })
  }

  @json(options: tagged)
  on GET(\"/kept\") () -> Effect[HttpResult[Kept]] by v: Visitor {
    Ok(Kept { role: Admin })
  }
}

service legacy from http {
  on GET(\"/legacy\") () -> Effect[HttpResult[Forced]] by v: Visitor {
    Ok(Forced { role: Admin })
  }

  @json(options: plain)
  on GET(\"/forced\") () -> Effect[HttpResult[Forced]] by v: Visitor {
    Ok(Forced { role: Admin })
  }
}
";

    #[test]
    fn a_type_reached_through_wrappers_and_payloads_is_written_by_the_route() {
        // Each hovered field is the first `role: Role` of its type's declaration.
        for (type_decl, what) in [
            ("type Inner = { role: Role", "through an `Option`"),
            (
                "type Item = { role: Role",
                "through a generic application's argument",
            ),
            ("type Member = { role: Role", "through a `Set`"),
            ("type Payload = { role: Role", "through a variant payload"),
        ] {
            let at =
                REACH.find(type_decl).expect("declared") + type_decl.len() - "role: Role".len();
            let nth = REACH[..at].matches("role: Role").count();
            let h = hover_in(REACH, "role: Role", nth).unwrap_or_else(|| panic!("{what}: hovers"));
            assert!(h.contains("`GET /envelope`"), "{what}: {h}");
            assert!(
                h.contains("a bare string, the variant's name"),
                "{what}: {h}"
            );
        }
    }

    #[test]
    fn a_route_level_json_option_overrides_its_service() {
        // `Kept` is answered only by `@json(options: tagged)` on a plain service: unchanged.
        let at = REACH.find("type Kept = { role: Role").unwrap() + "type Kept = { ".len();
        let nth = REACH[..at].matches("role: Role").count();
        let h = hover_in(REACH, "role: Role", nth).expect("the field hovers");
        assert!(!h.contains("plain-JSON route"), "{h}");
        // `Forced` is answered by a tagged-by-default service's `@json(options: plain)` route, and only that
        // one is named.
        let at = REACH.find("type Forced = { role: Role").unwrap() + "type Forced = { ".len();
        let nth = REACH[..at].matches("role: Role").count();
        let h = hover_in(REACH, "role: Role", nth).expect("the field hovers");
        assert!(h.contains("`GET /forced`") && !h.contains("/legacy"), "{h}");
    }

    #[test]
    fn an_option_and_a_list_say_so_and_many_routes_are_counted() {
        let h = hover_in(REACH, "at: Option", 0).expect("the field hovers");
        assert!(
            h.contains("RFC 3339 text in UTC") && h.contains("or `null` for `None`"),
            "{h}"
        );
        let h = hover_in(REACH, "roles: List", 0).expect("the field hovers");
        assert!(h.contains("an array of: a bare string"), "{h}");

        let mut many = String::from(
            "context shop.users\n\ntype Role = enum { Admin, Guest }\n\ntype U = { role: Role }\n\nservice s from http {\n  json { options: plain }\n",
        );
        for n in 1..=4 {
            many.push_str(&format!(
                "  on GET(\"/u{n}\") () -> Effect[HttpResult[U]] by v: Visitor {{\n    Ok(U {{ role: Admin }})\n  }}\n"
            ));
        }
        many.push_str("}\n");
        let h = hover_in(&many, "role: Role", 0).expect("the field hovers");
        assert!(
            h.contains("`GET /u1`, `GET /u2`, `GET /u3` and 1 more"),
            "{h}"
        );
    }
}
