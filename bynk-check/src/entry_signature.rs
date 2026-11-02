//! #2695: the signatures of a unit's entry handlers - every `on call(...)` of a
//! `service` - described structurally from the checker's resolved types, so a
//! host (the playground's Arguments row) never re-parses the source to learn
//! what a run can be passed.
//!
//! Each parameter's declared `TypeRef` is resolved with [`resolve_type_ref`]
//! (the checker's own resolution, against the unit's visible types) and the
//! resulting [`Ty`] is walked into an [`EntryShape`]; a named type's fields or
//! variants come from its `TypeDecl` in the same table. Plain data: the
//! consumer chooses the wire format (`bynk-wasm` renders JSON).

use std::collections::HashMap;
use std::sync::Arc;

use bynk_syntax::ast::{HandlerKind, ServiceProtocol, TypeBody, TypeDecl, TypeRef};

use crate::checker::{NamedKind, Ty, TyId, Types, resolve_type_ref};
use crate::symbols::UnitTable;

/// One `service`'s `on call(...)` handler.
#[derive(Debug, Clone, PartialEq)]
pub struct EntryHandler {
    /// The unit the service is declared in (`app.demo`).
    pub unit: String,
    pub service: String,
    pub params: Vec<EntryParam>,
    /// The declared return type, as the checker displays it.
    pub returns: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntryParam {
    pub name: String,
    /// The declared type, as the checker displays it (`Slug`, `List[Int]`).
    pub ty: String,
    pub shape: EntryShape,
}

/// What a value of a parameter's type looks like, resolved to its leaves.
#[derive(Debug, Clone, PartialEq)]
pub enum EntryShape {
    /// `Int`, `String`, `Bool`, `Float`, `Duration`, `Instant`, `Bytes`, `Decimal`.
    Base(&'static str),
    /// `type Slug = String where ...` - a value is a `base` value that passes
    /// the type's validator.
    Refined {
        name: String,
        base: &'static str,
    },
    /// `type Token = opaque String` - minted only through its own `.of`.
    Opaque {
        name: String,
        base: &'static str,
    },
    Record {
        name: String,
        fields: Vec<(String, EntryShape)>,
    },
    Sum {
        name: String,
        variants: Vec<(String, Vec<(String, EntryShape)>)>,
    },
    List(Box<EntryShape>),
    Set(Box<EntryShape>),
    Option(Box<EntryShape>),
    Map(Box<EntryShape>, Box<EntryShape>),
    Result(Box<EntryShape>, Box<EntryShape>),
    Unit,
    /// A type already being described further up (a self-referential record or
    /// sum): named, not expanded again.
    Recursive(String),
    /// Anything else (a generic record instantiation, a function, an effect),
    /// as the checker displays it. Not something a host can build a value for.
    Other(String),
}

/// The entry handlers of `table`'s services, ordered by service name then
/// declaration order. `types` is the unit's visible type table
/// ([`crate::symbols::combined_types_for`]).
pub fn entry_handlers(
    unit: &str,
    table: &UnitTable,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> Vec<EntryHandler> {
    let tys = Types::new();
    let mut names: Vec<&String> = table.services.keys().collect();
    names.sort();
    let mut out = Vec::new();
    for name in names {
        let svc = &table.services[name];
        if !matches!(svc.protocol, ServiceProtocol::Call) {
            continue;
        }
        for h in svc
            .handlers
            .iter()
            .filter(|h| matches!(h.kind, HandlerKind::Call))
        {
            let params = h
                .params
                .iter()
                .map(|p| {
                    let resolved = resolve_type_ref(&p.type_ref, types, &tys);
                    EntryParam {
                        name: p.name.name.clone(),
                        ty: display_or_unknown(resolved, &tys),
                        shape: shape_of_ref(resolved, types, &tys, &mut Vec::new()),
                    }
                })
                .collect();
            out.push(EntryHandler {
                unit: unit.to_string(),
                service: name.clone(),
                params,
                returns: display_or_unknown(resolve_type_ref(&h.return_type, types, &tys), &tys),
            });
        }
    }
    out
}

fn display_or_unknown(id: Option<TyId>, tys: &Types) -> String {
    id.map(|t| t.display(tys))
        .unwrap_or_else(|| "?".to_string())
}

fn shape_of_ref(
    id: Option<TyId>,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Types,
    open: &mut Vec<String>,
) -> EntryShape {
    match id {
        Some(t) => shape_of(t, types, tys, open),
        None => EntryShape::Other("?".to_string()),
    }
}

fn shape_of(
    id: TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Types,
    open: &mut Vec<String>,
) -> EntryShape {
    let ty = tys.get(id);
    let sub = |t: TyId, open: &mut Vec<String>| Box::new(shape_of(t, types, tys, open));
    match &*ty {
        Ty::Base(b) => EntryShape::Base(b.name()),
        Ty::Unit => EntryShape::Unit,
        Ty::List(t) => EntryShape::List(sub(*t, open)),
        Ty::Set(t) => EntryShape::Set(sub(*t, open)),
        Ty::Option(t) => EntryShape::Option(sub(*t, open)),
        Ty::Map(k, v) => EntryShape::Map(sub(*k, open), sub(*v, open)),
        Ty::Result(t, e) => EntryShape::Result(sub(*t, open), sub(*e, open)),
        Ty::Named { name, kind, args } => {
            if !args.is_empty() {
                return EntryShape::Other(id.display(tys));
            }
            match kind {
                NamedKind::Refined(b) => EntryShape::Refined {
                    name: name.clone(),
                    base: b.name(),
                },
                NamedKind::Opaque(b) => EntryShape::Opaque {
                    name: name.clone(),
                    base: b.name(),
                },
                NamedKind::Record | NamedKind::Sum => {
                    if open.contains(name) {
                        return EntryShape::Recursive(name.clone());
                    }
                    let Some(decl) = types.get(name) else {
                        return EntryShape::Other(id.display(tys));
                    };
                    open.push(name.clone());
                    let field = |tr: &TypeRef, open: &mut Vec<String>| {
                        shape_of_ref(resolve_type_ref(tr, types, tys), types, tys, open)
                    };
                    let shape = match &decl.body {
                        TypeBody::Record(r) => EntryShape::Record {
                            name: name.clone(),
                            fields: r
                                .fields
                                .iter()
                                .map(|f| (f.name.name.clone(), field(&f.type_ref, open)))
                                .collect(),
                        },
                        TypeBody::Sum(s) => EntryShape::Sum {
                            name: name.clone(),
                            variants: s
                                .variants
                                .iter()
                                .map(|v| {
                                    let payload = v
                                        .payload
                                        .iter()
                                        .map(|f| (f.name.name.clone(), field(&f.type_ref, open)))
                                        .collect();
                                    (v.name.name.clone(), payload)
                                })
                                .collect(),
                        },
                        _ => EntryShape::Other(id.display(tys)),
                    };
                    open.pop();
                    shape
                }
            }
        }
        _ => EntryShape::Other(id.display(tys)),
    }
}
