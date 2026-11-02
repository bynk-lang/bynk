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
use crate::symbols::{UnitTable, combined_types_for};

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
    /// A named record or sum left unexpanded because it sits deeper than
    /// [`MAX_DEPTH`] nested records/sums (a wide DAG of non-recursive types
    /// would otherwise grow exponentially).
    Named(String),
    /// Anything else (a generic record instantiation, a function, an effect),
    /// as the checker displays it. Not something a host can build a value for.
    Other(String),
}

/// How many records/sums may nest before a further one is reported as
/// [`EntryShape::Named`] instead of expanded.
pub const MAX_DEPTH: usize = 6;

type Scope = HashMap<String, Arc<TypeDecl>>;

/// Resolution scopes: a type's own fields are resolved in the scope of the unit
/// that *declares* it (its `combined_types_for`), not the entry unit's.
struct Scopes<'a> {
    unit_tables: &'a HashMap<String, UnitTable>,
    unit_uses: &'a HashMap<String, Vec<String>>,
    cache: HashMap<String, Arc<Scope>>,
    tys: Types,
}

impl Scopes<'_> {
    fn scope(&mut self, unit: &str) -> Arc<Scope> {
        let (t, u) = (self.unit_tables, self.unit_uses);
        self.cache
            .entry(unit.to_string())
            .or_insert_with(|| Arc::new(combined_types_for(unit, t, u)))
            .clone()
    }

    /// The unit declaring `decl` (found by identity among the unit tables),
    /// falling back to `visible_from`.
    fn declaring_unit(&self, name: &str, decl: &Arc<TypeDecl>, visible_from: &str) -> String {
        let mut units: Vec<&String> = self.unit_tables.keys().collect();
        units.sort();
        units
            .into_iter()
            .find(|u| {
                self.unit_tables[*u]
                    .types
                    .get(name)
                    .is_some_and(|d| Arc::ptr_eq(d, decl))
            })
            .cloned()
            .unwrap_or_else(|| visible_from.to_string())
    }
}

/// The entry handlers of the unit `unit`'s services, ordered by service name
/// then declaration order. Types are resolved in the entry unit's scope; the
/// fields of a named type in its declaring unit's.
pub fn entry_handlers(
    unit: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
) -> Vec<EntryHandler> {
    let Some(table) = unit_tables.get(unit) else {
        return Vec::new();
    };
    let mut sc = Scopes {
        unit_tables,
        unit_uses,
        cache: HashMap::new(),
        tys: Types::new(),
    };
    let entry_scope = sc.scope(unit);
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
                    let resolved = resolve_type_ref(&p.type_ref, &entry_scope, &sc.tys);
                    EntryParam {
                        name: p.name.name.clone(),
                        ty: display_or_unknown(resolved, &sc.tys),
                        shape: shape_of_ref(resolved, unit, &mut sc, &mut Vec::new()),
                    }
                })
                .collect();
            let returns = display_or_unknown(
                resolve_type_ref(&h.return_type, &entry_scope, &sc.tys),
                &sc.tys,
            );
            out.push(EntryHandler {
                unit: unit.to_string(),
                service: name.clone(),
                params,
                returns,
            });
        }
    }
    out
}

fn display_or_unknown(id: Option<TyId>, tys: &Types) -> String {
    id.map(|t| t.display(tys))
        .unwrap_or_else(|| "?".to_string())
}

/// `open` holds the (declaring unit, name) of the records/sums being expanded.
type Open = Vec<(String, String)>;

fn shape_of_ref(
    id: Option<TyId>,
    scope_unit: &str,
    sc: &mut Scopes,
    open: &mut Open,
) -> EntryShape {
    match id {
        Some(t) => shape_of(t, scope_unit, sc, open),
        None => EntryShape::Other("?".to_string()),
    }
}

/// `scope_unit` is the unit whose scope `id`'s named types are visible in.
fn shape_of(id: TyId, scope_unit: &str, sc: &mut Scopes, open: &mut Open) -> EntryShape {
    let ty = sc.tys.get(id);
    let sub =
        |t: TyId, sc: &mut Scopes, open: &mut Open| Box::new(shape_of(t, scope_unit, sc, open));
    match &*ty {
        Ty::Base(b) => EntryShape::Base(b.name()),
        Ty::Unit => EntryShape::Unit,
        Ty::List(t) => EntryShape::List(sub(*t, sc, open)),
        Ty::Set(t) => EntryShape::Set(sub(*t, sc, open)),
        Ty::Option(t) => EntryShape::Option(sub(*t, sc, open)),
        Ty::Map(k, v) => EntryShape::Map(sub(*k, sc, open), sub(*v, sc, open)),
        Ty::Result(t, e) => EntryShape::Result(sub(*t, sc, open), sub(*e, sc, open)),
        Ty::Named { name, kind, args } => {
            if !args.is_empty() {
                return EntryShape::Other(id.display(&sc.tys));
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
                    let visible = sc.scope(scope_unit);
                    let Some(decl) = visible.get(name) else {
                        return EntryShape::Other(id.display(&sc.tys));
                    };
                    let home = sc.declaring_unit(name, decl, scope_unit);
                    let key = (home.clone(), name.clone());
                    if open.contains(&key) {
                        return EntryShape::Recursive(name.clone());
                    }
                    if open.len() >= MAX_DEPTH {
                        return EntryShape::Named(name.clone());
                    }
                    open.push(key);
                    let decl = decl.clone();
                    let field = |tr: &TypeRef, sc: &mut Scopes, open: &mut Open| {
                        let scope = sc.scope(&home);
                        let r = resolve_type_ref(tr, &scope, &sc.tys);
                        shape_of_ref(r, &home, sc, open)
                    };
                    let shape = match &decl.body {
                        TypeBody::Record(r) => EntryShape::Record {
                            name: name.clone(),
                            fields: r
                                .fields
                                .iter()
                                .map(|f| (f.name.name.clone(), field(&f.type_ref, sc, open)))
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
                                        .map(|f| {
                                            (f.name.name.clone(), field(&f.type_ref, sc, open))
                                        })
                                        .collect();
                                    (v.name.name.clone(), payload)
                                })
                                .collect(),
                        },
                        _ => EntryShape::Other(id.display(&sc.tys)),
                    };
                    open.pop();
                    shape
                }
            }
        }
        _ => EntryShape::Other(id.display(&sc.tys)),
    }
}
