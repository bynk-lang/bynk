//! I4 (#1970): the typed TypeScript client for a context's foreign callers,
//! `workers/<context>/bynk-client.ts` (`design/archive/retired-tracks.md`, interop track D3).
//!
//! A Bynk caller of another context gets a typed call: the compiler builds the
//! path, stamps the contract hash, encodes the arguments and decodes the
//! result. This module gives a Worker that is not Bynk the same, as a module it
//! imports: one typed function per `on call` service a declared foreign caller
//! may call, the boundary types as TypeScript types, and the contract hashes
//! and wire version embedded as literals.
//!
//! # One codec, one caller
//!
//! Nothing here is a second implementation of the boundary. The codecs are the
//! ones a Bynk *consumer* generates for a callee's types
//! (`serialisation::emit_helpers_for_owner_qualified` under a non-empty
//! qualifier, which inlines a transparent refinement and casts an opaque
//! one), each call goes through the runtime's own `callService`, and the
//! encode and decode references at each call site come from the same
//! `serialisation::serialise_expr_via` / `serialisation::deserialise_ref_via`
//! the Bynk call site uses (`emit.rs`'s cross-context call lowering). The
//! runtime half ships as `@bynk/wire-client`, bundled from the runtime's own
//! modules and versioned with the wire format (§11 Q7).
//!
//! # Signing as a foreign caller
//!
//! I3's hand-off gives the foreign Worker its secret under the name a Bynk
//! caller holds its edge key by, `BYNK_CALL_TO_<CALLEE>`, and the callee's
//! verifier checks a declared foreign caller's name against its own secret
//! (`__FOREIGN_CALLERS`). So `callService` with the actor's name as the caller
//! signs exactly the request the callee admits. The client asks for the
//! foreign Worker's `env` (typed to require the secret) and its Service Binding.
//!
//! # When it is emitted
//!
//! Only for a context that declares at least one `Foreign` actor some `on call`
//! handler admits, and only with the services such an actor may call. A
//! context with no foreign caller has no one the client could sign as: an
//! undeclared caller holds no secret (§8 D2, §11 Q5).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use bynk_check::project_model::TypeDecl;
use bynk_check::resolver::{CrossContextService, cross_context_service_for};
use bynk_check::symbols::UnitTable;
use bynk_check::wire::{TypeBody, TypeRef};
use bynk_ts::{
    TsDecl, TsExpr, TsLit, TsObjectEntry, TsParam, TsProgram, TsStmt, TsType, TsTypeMember,
};

use super::runtime_use::RuntimeUse;
use super::serialisation;

/// The file, beside the Worker's `bynk-contracts.json`. Hyphenated like the
/// other compiler-written files there, so no Bynk unit can emit a module of
/// the same name.
pub const CLIENT_FILE: &str = "bynk-client.ts";

/// The runtime package the client imports (§11 Q7).
pub const CLIENT_PACKAGE: &str = "@bynk/wire-client";

/// The type names the client's own surface declares. A boundary type of the
/// same name is renamed in the client (`Client` → `Client_`), never the
/// surface, so the surface reads the same for every context.
const SURFACE_TYPES: &[&str] = &[
    "Admits",
    "Client",
    "ConnectOptions",
    "Env",
    "ForeignCaller",
    "Services",
];

/// The names `@bynk/wire-client` exports that generated code may use, each
/// with the import specifier that brings it in. The import line carries those
/// the module actually names ([`bynk_ts::unbound_names`]), so a caller whose
/// `tsconfig` sets `noUnusedLocals` builds it too.
const PACKAGE_NAMES: &[(&str, &str)] = &[
    ("Err", "Err"),
    ("None", "None"),
    ("Ok", "Ok"),
    ("Option", "type Option"),
    ("Result", "type Result"),
    ("Some", "Some"),
    ("ValidationError", "type ValidationError"),
    ("__BoundaryError", "type __BoundaryError"),
    ("__BynkKeyMap", "__BynkKeyMap"),
    ("__BynkKeySet", "__BynkKeySet"),
    ("__JsonValue", "type __JsonValue"),
    ("__ServiceBinding", "type __ServiceBinding"),
    ("__WireVersion", "type WireVersion as __WireVersion"),
    ("__boundaryError", "__boundaryError"),
    ("__bynkBytesFromBase64", "__bynkBytesFromBase64"),
    ("__bynkBytesToBase64", "__bynkBytesToBase64"),
    ("__callService", "__callService"),
];

/// One service the client exposes.
struct ClientService {
    svc: CrossContextService,
    /// The foreign actors its handler admits, sorted.
    admits: Vec<String>,
    hash: String,
}

/// The contract hash each declared foreign caller may call each service at:
/// actor → service → hash. What a foreign caller's generated client embeds,
/// so what `bynk deploy` compares a pin against. Empty when the context
/// admits no foreign caller.
pub fn foreign_contracts(
    table: &UnitTable,
    own_types: &HashMap<String, Arc<TypeDecl>>,
) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut out: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for s in client_services(table, own_types) {
        for actor in &s.admits {
            out.entry(actor.clone())
                .or_default()
                .insert(s.svc.name.clone(), s.hash.clone());
        }
    }
    out
}

/// The `on call` services a foreign caller may call, sorted by name.
fn client_services(
    table: &UnitTable,
    own_types: &HashMap<String, Arc<TypeDecl>>,
) -> Vec<ClientService> {
    let mut names: Vec<&String> = table.services.keys().collect();
    names.sort();
    let mut out = Vec::new();
    for name in names {
        let sdecl = &table.services[name];
        let Some(svc) = cross_context_service_for(name, sdecl) else {
            continue;
        };
        let mut admits: Vec<String> = sdecl
            .handlers
            .iter()
            .find_map(|h| bynk_check::actors::call_admission_for(h, &table.actors))
            .map(|a| a.foreign.into_iter().map(|f| f.actor).collect())
            .unwrap_or_default();
        if admits.is_empty() {
            continue;
        }
        admits.sort();
        admits.dedup();
        let hash = bynk_check::contract::service_contract_hash(&svc, own_types);
        out.push(ClientService { svc, admits, hash });
    }
    out
}

/// The client for `ctx_name`, or `None` when no foreign caller may call any of
/// its services. `own_types` is the context's own type namespace
/// (`combined_types_for`), the table its contract hashes are computed in.
pub(crate) fn emit_client(
    ctx_name: &str,
    table: &UnitTable,
    own_types: &HashMap<String, Arc<TypeDecl>>,
) -> Option<TsProgram> {
    let services = client_services(table, own_types);
    if services.is_empty() {
        return None;
    }
    let actors: BTreeSet<&String> = services.iter().flat_map(|s| s.admits.iter()).collect();

    // The codec closure of every exposed signature, in the callee's own
    // namespace: the same roots a Bynk consumer collects for the services it
    // calls (`emit_consumed_context_helpers`).
    let mut roots: Vec<TypeRef> = Vec::new();
    for s in &services {
        roots.extend(s.svc.params.iter().map(|(_, t)| t.clone()));
        roots.push(s.svc.return_type.clone());
    }
    let (closure, insts) = serialisation::collect_codec_closure(&roots, own_types);
    let mut generic_bases: BTreeSet<String> = BTreeSet::new();
    for i in &insts {
        if let serialisation::GenericInst::RecordInst { name, .. }
        | serialisation::GenericInst::SumInst { name, .. } = i
        {
            generic_bases.insert(name.clone());
        }
    }

    // A boundary type named like the client's surface is renamed here, in a
    // copy of the table, roots and closure; its contract hash was computed
    // above from the original, as the callee computes it.
    let mut declared: BTreeSet<String> = closure.iter().cloned().collect();
    declared.extend(generic_bases.iter().cloned());
    let mut subst: HashMap<String, String> = HashMap::new();
    for name in &declared {
        if SURFACE_TYPES.contains(&name.as_str()) {
            let mut to = name.clone();
            while SURFACE_TYPES.contains(&to.as_str()) || declared.contains(&to) {
                to.push('_');
            }
            subst.insert(name.clone(), to);
        }
    }
    let renamed = |t: &TypeRef| serialisation::rename_type_ref(t, &subst);
    let types: HashMap<String, Arc<TypeDecl>> = if subst.is_empty() {
        own_types.clone()
    } else {
        own_types
            .iter()
            .map(|(k, d)| {
                let decl = super::rename_in_decl(d, &subst);
                (
                    subst.get(k).cloned().unwrap_or_else(|| k.clone()),
                    Arc::new(decl),
                )
            })
            .collect()
    };
    let roots: Vec<TypeRef> = roots.iter().map(renamed).collect();
    let (names, insts) = serialisation::collect_codec_closure(&roots, &types);
    let generic_bases: BTreeSet<String> = generic_bases
        .into_iter()
        .map(|n| subst.get(&n).cloned().unwrap_or(n))
        .collect();

    // Every boundary type is declared in this module, under its own name, so
    // a whole-name qualifier (#1846's form) names it bare while still
    // selecting the consumer's codec: a transparent refinement re-checked
    // inline, an opaque one cast, since the owner's `.of` is not here.
    let mut qual: HashMap<String, String> = HashMap::new();
    for n in names.iter().chain(generic_bases.iter()) {
        qual.insert(n.clone(), n.clone());
    }

    let ru = RuntimeUse::default();
    let mut body: Vec<TsStmt> = Vec::new();

    // Constants.
    body.extend(doc_const(
        "The context this client calls.",
        "CONTEXT",
        None,
        str_lit(ctx_name),
    ));
    body.extend(doc_const(
        "The wire format version this client was generated for. Typed by the\n`@bynk/wire-client` package's own version, so a package that speaks another\nwire version fails `tsc` here.",
        "WIRE_VERSION",
        Some(TsType::named("__WireVersion")),
        str_lit(&bynk_check::wire_format::WIRE_VERSION.to_string()),
    ));
    let secret = bynk_check::wire_format::edge_key_to(ctx_name).unwrap_or_else(|| {
        panic!("bynk internal error: `{ctx_name}` is not an edge name, so it has no call key")
    });
    body.extend(doc_const(
        "The secret this Worker signs its calls with, as `bynk deploy` hands it over.",
        "SECRET",
        None,
        str_lit(&secret),
    ));
    let contract_entries: Vec<(String, TsExpr)> = services
        .iter()
        .map(|s| (s.svc.name.clone(), str_lit(&s.hash)))
        .collect();
    body.extend(doc_const(
        &contracts_doc(&services, ctx_name),
        "CONTRACTS",
        None,
        TsExpr::As {
            expr: Box::new(TsExpr::multiline_object(contract_entries)),
            ty: TsType::named("const"),
        },
    ));

    // The surface types.
    body.extend(doc(
        "The foreign callers the context declares, by actor name.",
        TsDecl::TypeAlias {
            name: "ForeignCaller".to_string(),
            type_params: Vec::new(),
            ty: TsType::union(actors.iter().map(|a| TsType::named(quoted(a))).collect()),
        },
    ));
    body.extend(doc(
        "What the calling Worker's `env` must hold.",
        TsDecl::Interface {
            name: "Env".to_string(),
            type_params: Vec::new(),
            members: vec![TsTypeMember::readonly_prop(
                secret.clone(),
                TsType::named("string"),
            )],
        },
    ));
    let mut service_members = Vec::new();
    for s in &services {
        service_members.push(TsTypeMember::method(
            s.svc.name.clone(),
            service_params(&s.svc, &renamed),
            service_return(&s.svc, &renamed),
        ));
    }
    body.extend(doc(
        "Every service a foreign caller of the context may call.",
        TsDecl::Interface {
            name: "Services".to_string(),
            type_params: Vec::new(),
            members: service_members,
        },
    ));
    body.extend(doc(
        "The foreign callers each service admits.",
        TsDecl::Interface {
            name: "Admits".to_string(),
            type_params: Vec::new(),
            members: services
                .iter()
                .map(|s| {
                    TsTypeMember::readonly_prop(
                        s.svc.name.clone(),
                        TsType::union(s.admits.iter().map(|a| TsType::named(quoted(a))).collect()),
                    )
                })
                .collect(),
        },
    ));
    body.extend(doc(
        "The services `A` may call: calling one its handler does not admit is a\n`tsc` error rather than a `403`.",
        TsDecl::TypeAlias {
            name: "Client".to_string(),
            type_params: vec!["A extends ForeignCaller".to_string()],
            ty: TsType::named(
                "{ readonly [S in keyof Services as [A] extends [Admits[S]] ? S : never]: Services[S] }",
            ),
        },
    ));
    body.extend(doc(
        "How to reach the context: the actor this Worker is declared as, its Service\nBinding to the context's Worker, and its `env`, which holds `SECRET`.",
        TsDecl::Interface {
            name: "ConnectOptions".to_string(),
            type_params: vec!["A extends ForeignCaller".to_string()],
            members: vec![
                TsTypeMember::readonly_prop("caller", TsType::named("A")),
                TsTypeMember::readonly_prop("binding", TsType::named("__ServiceBinding")),
                TsTypeMember::readonly_prop("env", TsType::named("Env")),
            ],
        },
    ));

    // `connect`.
    let opt = |p: &str| TsExpr::Member {
        object: Box::new(TsExpr::Ident("options".to_string())),
        property: p.to_string(),
    };
    let mut methods = Vec::new();
    for s in &services {
        let encoded: Vec<TsExpr> = s
            .svc
            .params
            .iter()
            .map(|(name, t)| {
                serialisation::serialise_expr_via(&renamed(t), &super::ts_ident(name), "", &ru)
            })
            .collect();
        let args = match encoded.len() {
            0 => TsExpr::object_entries(Vec::new()),
            1 => encoded.into_iter().next().expect("one argument"),
            _ => TsExpr::object_entries(
                s.svc
                    .params
                    .iter()
                    .zip(encoded)
                    .map(|((name, _), e)| TsObjectEntry::Prop(name.clone(), e))
                    .collect(),
            ),
        };
        let call = TsExpr::Call {
            callee: Box::new(TsExpr::Ident("__callService".to_string())),
            args: vec![
                opt("binding"),
                str_lit(&s.svc.name),
                args,
                serialisation::deserialise_ref_via(&renamed(&s.svc.return_type), "", &ru),
                opt("caller"),
                TsExpr::Member {
                    object: Box::new(TsExpr::Ident("CONTRACTS".to_string())),
                    property: s.svc.name.clone(),
                },
                opt("env"),
                TsExpr::Ident("CONTEXT".to_string()),
            ],
        };
        methods.push(TsObjectEntry::Method {
            name: s.svc.name.clone(),
            is_async: false,
            generics: Vec::new(),
            params: service_params(&s.svc, &renamed),
            return_type: Some(service_return(&s.svc, &renamed)),
            doc: None,
            inline: false,
            body: vec![TsStmt::return_stmt(Some(call), None)],
        });
    }
    body.extend(doc(
        "A client calling the context as the foreign caller `options.caller`. Each\ncall is signed under `SECRET`, stamped with its service's contract hash,\nand decoded with the codec a Bynk caller uses. A refusal or transport failure\nis thrown; `callErrorOf` (from `@bynk/wire-client`) reads it.\n\n    const client = connect({ caller: \"<Actor>\", binding: env.<BINDING>, env });",
        TsDecl::Function {
            name: "connect".to_string(),
            generics: vec!["A extends ForeignCaller".to_string()],
            params: vec![TsParam {
                name: "options".to_string(),
                ty: Some(TsType::named_with_args(
                    "ConnectOptions",
                    vec![TsType::named("A")],
                )),
                optional: false,
            }],
            return_type: Some(TsType::named_with_args("Client", vec![TsType::named("A")])),
            body: vec![
                TsStmt::const_stmt(
                    bynk_ts::TsBindingName::Ident("services".to_string()),
                    Some(TsType::named("Services")),
                    TsExpr::multiline_object_entries(methods),
                    None,
                ),
                TsStmt::return_stmt(
                    Some(TsExpr::As {
                        expr: Box::new(TsExpr::As {
                            expr: Box::new(TsExpr::Ident("services".to_string())),
                            ty: TsType::named("unknown"),
                        }),
                        ty: TsType::named_with_args("Client", vec![TsType::named("A")]),
                    }),
                    None,
                ),
            ],
            is_async: false,
            inline: false,
        },
    ));

    // The boundary types, then their codecs.
    let mut type_names: Vec<String> = names
        .iter()
        .filter(|n| types.get(*n).is_some_and(|d| d.type_params.is_empty()))
        .cloned()
        .collect();
    type_names.extend(generic_bases.iter().cloned());
    type_names.sort();
    type_names.dedup();
    for name in &type_names {
        if let Some(decl) = types.get(name) {
            body.extend(boundary_type(name, decl));
        }
    }
    body.extend(serialisation::decls_as_stmts(
        serialisation::emit_helpers_for_owner_qualified(&names, &types, ctx_name, &qual, &ru),
    ));
    body.extend(serialisation::decls_as_stmts(
        serialisation::emit_generic_helpers_qualified(&insts, &types, &qual, &ru),
    ));

    // The header and the import line, carrying the package names the module
    // actually uses.
    let unbound = bynk_ts::unbound_names(&TsProgram {
        stmts: body.clone(),
    });
    let imports: Vec<String> = PACKAGE_NAMES
        .iter()
        .filter(|(name, _)| unbound.iter().any(|u| u == name))
        .map(|(_, spec)| spec.to_string())
        .collect();
    let mut stmts = vec![
        TsStmt::comment("Generated by bynkc — do not edit by hand.", None),
        TsStmt::comment(
            [
                "The typed client for context `",
                ctx_name,
                "`, for a Worker that is not Bynk.",
            ]
            .concat(),
            None,
        ),
        TsStmt::comment(
            "It imports `@bynk/wire-client`: https://bynk-lang.org/book/reference/typescript-client/",
            None,
        ),
        TsStmt::decl(
            TsDecl::Import {
                type_only: false,
                names: imports,
                from: CLIENT_PACKAGE.to_string(),
            },
            None,
        ),
    ];
    stmts.extend(body);
    Some(TsProgram { stmts })
}

/// `CONTRACTS`' doc: what the hashes are, and the pin the callee's project
/// records for each foreign caller (`bynk.foreign.toml`, §8 D3 layer 2), ready
/// to paste.
fn contracts_doc(services: &[ClientService], ctx_name: &str) -> String {
    let mut lines = vec![
        "The contract hash of each service, as the callee was built. A call stamps".to_string(),
        "it, and the callee refuses a different one with `409 ContractMismatch`.".to_string(),
        String::new(),
        "The context's project pins them per foreign caller in `bynk.foreign.toml`,".to_string(),
        "beside its `bynk.toml`, and `bynk deploy` refuses a contract change that".to_string(),
        "breaks a pin. Regenerate this client, then update the pin:".to_string(),
    ];
    let mut by_actor: BTreeMap<&String, Vec<&ClientService>> = BTreeMap::new();
    for s in services {
        for a in &s.admits {
            by_actor.entry(a).or_default().push(s);
        }
    }
    for (actor, svcs) in by_actor {
        lines.push(String::new());
        lines.push(["    [\"", ctx_name, "\".", actor, "]"].concat());
        for s in svcs {
            lines.push(["    ", &s.svc.name, " = \"", &s.hash, "\""].concat());
        }
    }
    lines.join("\n")
}

/// A service's parameters, typed for the client.
fn service_params(
    svc: &CrossContextService,
    renamed: &dyn Fn(&TypeRef) -> TypeRef,
) -> Vec<TsParam> {
    svc.params
        .iter()
        .map(|(name, t)| TsParam {
            name: super::ts_ident(name),
            ty: Some(ts_type(&renamed(t))),
            optional: false,
        })
        .collect()
}

/// A service's result, as the promise a client call returns.
fn service_return(svc: &CrossContextService, renamed: &dyn Fn(&TypeRef) -> TypeRef) -> TsType {
    let ret = match renamed(&svc.return_type) {
        TypeRef::Effect(inner, _) => *inner,
        other => other,
    };
    TsType::named_with_args("globalThis.Promise", vec![ts_type(&ret)])
}

/// The TypeScript type of a boundary type reference, every name bare.
fn ts_type(t: &TypeRef) -> TsType {
    super::ts_type_ref_qualified_multi_ts_type(t, &HashMap::new())
}

/// A boundary type's declaration in the client: its in-memory shape, which is
/// what the codecs produce and consume. A record is an interface of readonly
/// fields; a sum a union discriminated by `tag`; a transparent refined type
/// its base type (the callee checks the predicate); an opaque type its base
/// type branded, so a caller passes back a value it was given rather than
/// making one.
fn boundary_type(name: &str, decl: &TypeDecl) -> Vec<TsStmt> {
    let params: Vec<String> = decl
        .type_params
        .iter()
        .map(|p| p.name.name.clone())
        .collect();
    let d = match &decl.body {
        TypeBody::Record(r) => TsDecl::Interface {
            name: name.to_string(),
            type_params: params,
            members: r
                .fields
                .iter()
                .map(|f| TsTypeMember::readonly_prop(f.name.name.clone(), ts_type(&f.type_ref)))
                .collect(),
        },
        TypeBody::Sum(sum) => TsDecl::TypeAlias {
            name: name.to_string(),
            type_params: params,
            ty: TsType::multiline_union(
                sum.variants
                    .iter()
                    .map(|v| {
                        let mut members = vec![TsTypeMember::readonly_prop(
                            "tag",
                            TsType::named(quoted(&v.name.name)),
                        )];
                        members.extend(v.payload.iter().map(|f| {
                            TsTypeMember::readonly_prop(
                                super::payload_prop(&f.name.name),
                                ts_type(&f.type_ref),
                            )
                        }));
                        TsType::Object(members)
                    })
                    .collect(),
            ),
        },
        TypeBody::Refined {
            base, base_span, ..
        } => TsDecl::TypeAlias {
            name: name.to_string(),
            type_params: params,
            ty: ts_type(&TypeRef::Base(*base, *base_span)),
        },
        TypeBody::Opaque {
            base, base_span, ..
        } => TsDecl::TypeAlias {
            name: name.to_string(),
            type_params: params,
            ty: TsType::intersection(vec![
                ts_type(&TypeRef::Base(*base, *base_span)),
                TsType::Object(vec![TsTypeMember::readonly_prop(
                    "__opaque",
                    TsType::named(quoted(name)),
                )]),
            ]),
        },
    };
    let mut out = Vec::new();
    if let Some(text) = decl.documentation.as_deref() {
        out.push(TsStmt::doc_comment(text, None));
    }
    let mut stmt = TsStmt::decl(TsDecl::Export(Box::new(d)), None);
    stmt.no_blank_before = !out.is_empty();
    out.push(stmt);
    out
}

fn str_lit(s: &str) -> TsExpr {
    TsExpr::Lit(TsLit::Str(s.to_string()))
}

/// `"name"`, a string literal type. Actor, variant and type names are
/// identifiers, so there is nothing to escape.
fn quoted(name: &str) -> String {
    ["\"", name, "\""].concat()
}

/// An exported declaration under a doc comment.
fn doc(text: &str, decl: TsDecl) -> Vec<TsStmt> {
    let mut stmt = TsStmt::decl(TsDecl::Export(Box::new(decl)), None);
    stmt.no_blank_before = true;
    vec![TsStmt::doc_comment(text, None), stmt]
}

fn doc_const(text: &str, name: &str, ty: Option<TsType>, init: TsExpr) -> Vec<TsStmt> {
    doc(
        text,
        TsDecl::ConstDecl {
            name: name.to_string(),
            ty,
            init,
        },
    )
}
