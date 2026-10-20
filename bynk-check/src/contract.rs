//! v0.177 (#643): the canonical normal form of a cross-context contract, and
//! its hash.
//!
//! A `workers` build compiles context A against context B's contract, and
//! nothing at runtime checks that the *deployed* B still matches what A was
//! compiled against — `deploy --context NAME` institutionalises the skew. The
//! fix is to stamp a hash of the compiled contract beside `X-Bynk-Caller` and
//! fail closed on mismatch (ADR 0092's pattern: a compile-time constant in a
//! reserved header, metadata beside the payload, no crypto).
//!
//! The hash is only as good as the form it hashes. Two rules make it usable:
//!
//! 1. **Semantically-equal contracts must hash equal**, or a working deployment
//!    409s spuriously — which is worse than no check at all, because it breaks
//!    what worked and destroys trust in the mechanism. This is why the form is
//!    canonical (predicates as a sorted set, record fields sorted by name)
//!    rather than a rendering of source order.
//! 2. **Both sides must canonicalise the *same* thing.** The callee's contract
//!    is canonicalised **in the callee's own namespace**, from the callee's own
//!    type table, on *both* sides — never in the caller's. The caller reaches
//!    that table through `consumed_types[callee]` and the callee through its own
//!    combined table; both are produced by the same `combined_types_for`, so the
//!    two views cannot diverge by construction. A caller never canonicalises a
//!    consumed type in its own namespace, where its rebranding would make the
//!    same type render differently.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;

use bynk_syntax::ast::{Ident, PredKind, Refinement, TypeBody, TypeDecl, TypeRef};
use bynk_syntax::span::Span;

use crate::resolver::{CrossContextService, cross_context_service_for};
use crate::symbols::UnitTable;
use crate::unit_signature::{
    CapabilityOpSignature, FnSignature, HandlerKindSignature, HandlerSignature,
    MethodTableSignature, ParamSignature, ProtocolSignature, StoreFieldSignature, UnitSignature,
};

/// The canonical normal form of one `on call` service contract.
///
/// Shape: `<service>(<param>: <type>, …) -> <type>`. Parameter **names** and
/// **order** are both included, and both are load-bearing rather than cosmetic:
/// a multi-argument call sends an object keyed by parameter name, and a
/// single-argument call sends the bare value — so a rename or a reorder is a
/// genuine wire change, not a refactor.
pub fn service_normal_form(
    svc: &CrossContextService,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> String {
    let mut out = String::new();
    let _ = write!(out, "{}(", svc.name);
    for (i, (pname, pty)) in svc.params.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        let _ = write!(
            out,
            "{pname}: {}",
            canon_type(pty, types, &mut HashSet::new())
        );
    }
    let _ = write!(
        out,
        ") -> {}",
        canon_type(&svc.return_type, types, &mut HashSet::new())
    );
    out
}

/// The canonical form of a type *as it appears on the wire*.
///
/// A named type expands **structurally**, not by name alone: the wire carries
/// the fields, so renaming a record field or changing a variant's payload is a
/// contract change that a name-only form would miss entirely. The name is kept
/// alongside the structure because Bynk's types are nominal — swapping
/// `AuthId` for a structurally identical `SessionId` changes the contract even
/// though the bytes are unchanged. Keeping the name costs nothing in false
/// positives: a rename already breaks the consumer's *compile*, so it cannot
/// reach a deploy without the consumer being rebuilt too.
fn canon_type(
    t: &TypeRef,
    types: &HashMap<String, Arc<TypeDecl>>,
    seen: &mut HashSet<String>,
) -> String {
    canon_type_in(t, types, seen, &HashMap::new(), Mode::Contract)
}

/// `subst` binds a generic declaration's type-parameter **names** to the
/// canonical form of the concrete argument supplied at the use site.
///
/// A generic body MUST expand with its parameters substituted, or the
/// parameter's *name* leaks into the form: `type Page[T] = { items: List[T] }`
/// and the same declaration spelled with `U` are the same type with the same
/// wire shape, but would hash differently. Across a deploy that renames a type
/// parameter — a pure refactor with no wire consequence — every call would 409.
/// That is the same class of spurious failure the sorted fields and the
/// predicate set exist to prevent, and it is the one the module's own standard
/// ("semantically-equal contracts must hash equal") forbids.
fn canon_type_in(
    t: &TypeRef,
    types: &HashMap<String, Arc<TypeDecl>>,
    seen: &mut HashSet<String>,
    subst: &HashMap<String, String>,
    mode: Mode,
) -> String {
    match t {
        TypeRef::Base(b, _) => b.name().to_string(),
        TypeRef::Unit(_) => "()".to_string(),
        // An `Effect` wraps the handler, not the payload — the caller awaits the
        // promise, so it is not part of the wire contract.
        TypeRef::Effect(inner, _) => canon_type_in(inner, types, seen, subst, mode),
        TypeRef::List(a, _) => format!("List[{}]", canon_type_in(a, types, seen, subst, mode)),
        TypeRef::Option(a, _) => format!("Option[{}]", canon_type_in(a, types, seen, subst, mode)),
        TypeRef::Result(a, b, _) => format!(
            "Result[{}, {}]",
            canon_type_in(a, types, seen, subst, mode),
            canon_type_in(b, types, seen, subst, mode)
        ),
        TypeRef::Map(k, v, _) => format!(
            "Map[{}, {}]",
            canon_type_in(k, types, seen, subst, mode),
            canon_type_in(v, types, seen, subst, mode)
        ),
        // Generic-record instantiation: the arguments are positional, so their
        // order *is* semantic and is preserved (unlike a record's fields).
        TypeRef::App { name, args, .. } => {
            let inner: Vec<String> = args
                .iter()
                .map(|a| canon_type_in(a, types, seen, subst, mode))
                .collect();
            // Bind the declaration's parameters to these arguments so the body
            // expands over concrete types and the parameter's name never reaches
            // the form.
            let bound: HashMap<String, String> = types
                .get(&name.name)
                .map(|d| {
                    d.type_params
                        .iter()
                        .zip(&inner)
                        .map(|(p, a)| (p.name.name.clone(), a.clone()))
                        .collect()
                })
                .unwrap_or_default();
            let head = canon_named_in(&name.name, types, seen, &bound, mode);
            format!("{head}[{}]", inner.join(", "))
        }
        TypeRef::Named(id) => {
            // A bound type parameter renders as the argument it stands for.
            match subst.get(&id.name) {
                Some(bound) => bound.clone(),
                None => canon_named_in(&id.name, types, seen, subst, mode),
            }
        }
        TypeRef::HttpResult(a, _) => {
            format!("HttpResult[{}]", canon_type_in(a, types, seen, subst, mode))
        }
        TypeRef::ValidationError(_) => "ValidationError".to_string(),
        TypeRef::JsonError(_) => "JsonError".to_string(),
        TypeRef::QueueResult(_) => "QueueResult".to_string(),
        TypeRef::Asset(_) => "Asset".to_string(),
        // The confined family is rejected at every boundary, so it cannot appear
        // in a contract. Render it rather than panic: the normal form is also a
        // diagnostic surface, and a compiler bug should not become a crash here.
        TypeRef::Fn(..)
        | TypeRef::Query(..)
        | TypeRef::Stream(..)
        | TypeRef::Connection(..)
        | TypeRef::History(..) => "<non-boundary>".to_string(),
    }
}

fn canon_named_in(
    name: &str,
    types: &HashMap<String, Arc<TypeDecl>>,
    seen: &mut HashSet<String>,
    subst: &HashMap<String, String>,
    mode: Mode,
) -> String {
    // A recursive record terminates on the data, so its codec is finite and it
    // is a legal contract — but its *expansion* is not. Emit a back-reference on
    // revisit. `type Node = { next: Option[Node] }` canonicalises as
    // `Node{next: Option[@Node]}` — the cycle is named, so two different
    // recursive shapes still differ.
    if !seen.insert(name.to_string()) {
        return format!("@{name}");
    }
    let Some(decl) = types.get(name) else {
        // Not in the callee's table: a runtime- or compiler-known name with no
        // declaration to expand. The name alone is the whole contract for it.
        seen.remove(name);
        return name.to_string();
    };
    let body = match &decl.body {
        // Record fields sort by name: a JSON object is unordered, so field
        // *order* is not wire-observable and must not perturb the hash — while
        // field *presence* and type are exactly what the hash exists to pin.
        //
        // #1969: an inline refinement is part of the field's type, so it is
        // part of the form (`qty: Int where Positive`), canonicalised as a
        // sorted set exactly like a named refined type's. The decoder checks
        // it, so a callee that tightens it rejects values its callers send.
        TypeBody::Record(r) => {
            let mut fields: Vec<String> = r
                .fields
                .iter()
                .map(|f| {
                    let ty = canon_type_in(&f.type_ref, types, seen, subst, mode);
                    match f.refinement.as_ref().filter(|_| mode.keeps_predicates()) {
                        Some(r) => format!("{}: {ty} {}", f.name.name, canon_refinement(Some(r))),
                        None => format!("{}: {ty}", f.name.name),
                    }
                })
                .collect();
            fields.sort();
            format!("{{{}}}", fields.join(", "))
        }
        // Variants sort by name for the same reason: the wire carries a `kind`
        // discriminant, so declaration order is invisible to it.
        TypeBody::Sum(s) => {
            let mut variants: Vec<String> = s
                .variants
                .iter()
                .map(|v| {
                    let payload: Vec<String> = v
                        .payload
                        .iter()
                        .map(|p| canon_type_in(&p.type_ref, types, seen, subst, mode))
                        .collect();
                    if payload.is_empty() {
                        v.name.name.clone()
                    } else {
                        format!("{}({})", v.name.name, payload.join(", "))
                    }
                })
                .collect();
            variants.sort();
            format!("|{}", variants.join("|"))
        }
        TypeBody::Refined {
            base, refinement, ..
        } => {
            if mode.keeps_predicates() {
                format!("{} {}", base.name(), canon_refinement(refinement.as_ref()))
            } else {
                base.name().to_string()
            }
        }
        // In the **contract** form, an opaque type's predicate is deliberately
        // excluded — only its representation is part of the contract.
        //
        // The consumer cannot see the predicate by construction (that is what
        // `exports opaque` means), so no consumer behaviour can depend on it: it
        // can hold and pass an `AuthId`, never inspect or mint one. Including the
        // predicate would therefore manufacture skew failures between two
        // contexts that cannot disagree — the owner tightening `Matches(...)`
        // would 409 every caller for a change none of them can observe. This is
        // the same position ADR 0199 took on opacity, from the same premise.
        //
        // Stored state is the other way round (state-migrations S0, #2204): the
        // owner re-validates its own opaque values on rehydration, so a
        // tightened opaque predicate *is* observable there, and the full
        // storage rendering keeps it. The stored bytes are just the base, so
        // neither storage rendering marks opacity.
        TypeBody::Opaque {
            base, refinement, ..
        } => match mode {
            Mode::Contract => format!("{} opaque", base.name()),
            Mode::StorageStructure => base.name().to_string(),
            Mode::StorageFull => {
                format!("{} {}", base.name(), canon_refinement(refinement.as_ref()))
            }
        },
    };
    seen.remove(name);
    match mode {
        Mode::Contract => format!("{name}{body}"),
        // Stored bytes carry no type names (records are keyed by field, sums
        // tagged by variant), so renaming a type must not read as a retyped
        // field. The name survives only as a recursion anchor (`@Name` above).
        Mode::StorageStructure | Mode::StorageFull => body,
    }
}

/// Which canonical form a walk renders.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// The cross-context contract form: nominal, refinements kept, opaque
    /// predicates elided. Backs [`contract_hash`].
    Contract,
    /// A stored field's structure (state-migrations S0): structural, every
    /// predicate stripped. A change here changes what is on disk.
    StorageStructure,
    /// A stored field's structure *and* refinements, opaque ones included. A
    /// change here alone leaves the stored bytes readable.
    StorageFull,
}

impl Mode {
    fn keeps_predicates(self) -> bool {
        !matches!(self, Mode::StorageStructure)
    }
}

/// State-migrations S0 (#2204, track §3.1): the two canonical renderings of a
/// stored type that `bynk.schema.lock`'s `[agents]` table records — the
/// structure (`.0`, predicates stripped) and the full form (`.1`, predicates
/// kept, opaque ones included). Both expand named records and sums
/// structurally, sort fields and variants by name, and close recursion with an
/// `@Name` back-reference, exactly as the contract form does; unlike it, they
/// carry no type names, because stored bytes carry none.
pub fn canon_storage_type(t: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>) -> (String, String) {
    let render = |mode| canon_type_in(t, types, &mut HashSet::new(), &HashMap::new(), mode);
    (render(Mode::StorageStructure), render(Mode::StorageFull))
}

/// Predicates canonicalise as a **sorted set**.
///
/// This is not a nicety adjacent to the hash; it is a precondition for it.
/// Predicates are conjunctive and side-effect-free, so `String where NonEmpty,
/// MaxLen(10)` and `String where MaxLen(10), NonEmpty` are the *same type* — and
/// hashing them in source order would make two contexts that agree perfectly
/// fail closed against each other. The same normal form also backs the checker's
/// `refinements_match`, so the matcher and the hash cannot disagree about what
/// "the same refinement" means.
pub fn canon_refinement(r: Option<&Refinement>) -> String {
    let Some(r) = r else {
        return String::new();
    };
    let mut preds: Vec<String> = r
        .predicates
        .iter()
        .map(|p| canon_predicate(&p.kind))
        .collect();
    preds.sort();
    preds.dedup();
    format!("where {}", preds.join(", "))
}

pub fn canon_predicate(p: &PredKind) -> String {
    match p {
        PredKind::Matches(s) => format!("Matches({s:?})"),
        // Bounds keep their source lexemes elsewhere (byte-stable emission), but
        // a contract is about *values*: `1` and `01` are the same bound, so the
        // parsed value is what canonicalises.
        PredKind::InRange(a, b) => format!("InRange({}, {})", a.value, b.value),
        PredKind::InRangeF(a, b) => format!("InRangeF({}, {})", a.value, b.value),
        PredKind::MinLength(n) => format!("MinLength({n})"),
        PredKind::MaxLength(n) => format!("MaxLength({n})"),
        PredKind::Length(n) => format!("Length({n})"),
        // R12.2 names these as sugar for `InRange(0, ∞)`/`InRange(1, ∞)`, but the
        // fold stays undone (#1049): neither base has a writable literal bound
        // that stands for `∞` — the lexer rejects any float literal that would
        // parse to infinity, and `Int`'s `i64::MAX` is a real, arbitrary finite
        // bound, not the language's spelling of "unbounded". Folding onto an
        // invented string nothing else can produce would only rename the
        // literal, at the cost of a contract-hash change for every boundary
        // type carrying one. Revisit alongside R12.3 (entailment), which is
        // the actual consumer of a normalised Interval domain.
        PredKind::NonNegative => "NonNegative".to_string(),
        PredKind::Positive => "Positive".to_string(),
        // `NonEmpty` is sugar for `MinLength(1)` (R12.2) — folding it here makes
        // `String where NonEmpty` and `String where MinLength(1)` the same
        // canonical form, so `service_contract_hash` and `refinements_match`
        // agree that they are the same type. Unconditional: `NonEmpty` only
        // ever applies to `BaseType::String` (`refinements.rs`'s
        // `pred_applies_to`), so no base needs threading through here.
        PredKind::NonEmpty => "MinLength(1)".to_string(),
    }
}

/// FNV-1a (64-bit) over the canonical form, rendered as 16 lowercase hex chars.
///
/// **Why not a cryptographic hash.** Trust here is static and channel-based, and
/// this increment does not change that (ADR 0092): `/_bynk/call/` is
/// platform-dispatched and not externally routable, every context in a
/// deployment is one trust domain, and a malicious first-party context is out of
/// the threat model. This is a **skew detector, not a security control** — an
/// accident detector. Forging it buys an attacker nothing they could not already
/// do, so `sha2`'s ~6-crate dependency tree would buy nothing either. A
/// collision degrades to *today's* behaviour for that one pair (an undetected
/// skew), not to something worse, and at ~1e-14 for a 1000-contract project it
/// is not the risk worth engineering against.
///
/// **Why hand-rolled.** `std::collections::hash_map::DefaultHasher` is
/// explicitly not stable across Rust releases, so it cannot back a value that
/// crosses a wire or is compared between two separately-compiled binaries. FNV-1a
/// is fully specified, so two compilers agree forever.
pub fn contract_hash(normal_form: &str) -> String {
    fnv1a64_hex(normal_form.as_bytes())
}

/// FNV-1a 64-bit offset basis. Published in the wire-format reference (#1969).
pub const FNV1A64_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a 64-bit prime. Published in the wire-format reference (#1969).
pub const FNV1A64_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a (64-bit) over `bytes`, as 16 lowercase hex chars — [`contract_hash`]'s
/// algorithm over raw bytes. #1883, slice 1b also uses it for an asset's
/// content hash (`assets::served_path`), where the same reasoning holds: it
/// only has to tell one project's own files apart, reproducibly.
pub fn fnv1a64_hex(bytes: &[u8]) -> String {
    let mut h = FNV1A64_OFFSET;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(FNV1A64_PRIME);
    }
    format!("{h:016x}")
}

/// The stamped contract hash for one consumed service.
pub fn service_contract_hash(
    svc: &CrossContextService,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> String {
    contract_hash(&service_normal_form(svc, types))
}

/// v0.177 (#643): a context's own `on call` contract hashes, keyed by service
/// name — the constants its Worker entry compares an incoming
/// `X-Bynk-Contract` against.
///
/// Built by projecting each local `on call` handler into the **same**
/// [`CrossContextService`] shape [`crate::symbols::build_cross_context_info`]
/// hands a *caller* for the same service, via the one shared projection
/// [`cross_context_service_for`], and hashing it from the same combined type
/// table. That symmetry is the whole correctness argument: a caller and a
/// callee compiled from one source tree must agree, or the check fires on
/// every call instead of only on real skew — sharing the projection makes the
/// agreement structural rather than two hand-written copies staying in sync
/// by convention.
pub fn own_contract_hashes(
    table: &UnitTable,
    own_types: &HashMap<String, Arc<TypeDecl>>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (sname, sdecl) in &table.services {
        let Some(svc) = cross_context_service_for(sname, sdecl) else {
            continue;
        };
        out.insert(sname.clone(), service_contract_hash(&svc, own_types));
    }
    out
}

/// P8.1 (#1512, [DECISION C]): the canonical form of a body-free parameter
/// list — shared by [`canon_fn_signature`] and [`canon_handler_signature`] so
/// the two categories can't drift on how a parameter renders.
fn canon_params(params: &[ParamSignature], types: &HashMap<String, Arc<TypeDecl>>) -> String {
    let mut out = String::new();
    for (i, p) in params.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        let _ = write!(
            out,
            "{}: {}",
            p.name,
            canon_type(&p.type_ref, types, &mut HashSet::new())
        );
    }
    out
}

/// P8.1 (#1512, [DECISION C]): the canonical form of a [`FnSignature`] —
/// extends this module's own `canon_type` (ADR 0200) to a shape it doesn't
/// reach today. Body-free by construction ([DECISION B]): there is no
/// `body`/`requires`/`ensures` field on `FnSignature` to accidentally
/// include here.
pub fn canon_fn_signature(f: &FnSignature, types: &HashMap<String, Arc<TypeDecl>>) -> String {
    let mut out = String::new();
    let _ = write!(out, "fn {}", f.name);
    if !f.type_params.is_empty() {
        let _ = write!(out, "[{}]", f.type_params.join(", "));
    }
    let _ = write!(out, "({})", canon_params(&f.params, types));
    let _ = write!(
        out,
        " -> {}",
        canon_type(&f.return_type, types, &mut HashSet::new())
    );
    if f.has_self {
        out.push_str(" self");
    }
    out
}

/// P8.1 (#1512, [DECISION C]): the canonical form of a [`HandlerKindSignature`].
/// Added alongside the rest of [`canon_handler_signature`] by PR #1517's own
/// bot review (finding #1) — every field here is already a plain value
/// (`HttpMethod`'s own rendered name, a route/cron `String`), so nothing
/// needs excluding beyond the enum discriminant itself carrying no `Span`.
fn canon_handler_kind(k: &HandlerKindSignature) -> String {
    match k {
        HandlerKindSignature::Call => "call".to_string(),
        HandlerKindSignature::Http { method, path } => format!("http {method} {path:?}"),
        HandlerKindSignature::Cron { expr } => format!("cron {expr:?}"),
        HandlerKindSignature::Message => "message".to_string(),
        HandlerKindSignature::Open => "open".to_string(),
        HandlerKindSignature::Close => "close".to_string(),
        HandlerKindSignature::Event => "event".to_string(),
    }
}

/// P8.1 (#1512, [DECISION C]): the canonical form of a [`HandlerSignature`].
/// `given` renders sorted — its rendered `CapRef` names have no meaningful
/// order to preserve (it's a set, not a sequence). `kind` is rendered first
/// (PR #1517's own bot review, finding #1) — two handlers that differ only
/// in HTTP method or route, or in `on call` vs. `on message`, must not
/// canonicalise identically.
pub fn canon_handler_signature(
    h: &HandlerSignature,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> String {
    let mut out = String::new();
    let _ = write!(out, "on {}", canon_handler_kind(&h.kind));
    if let Some(m) = &h.method_name {
        let _ = write!(out, " {m}");
    }
    let _ = write!(out, "({})", canon_params(&h.params, types));
    let _ = write!(
        out,
        " -> {}",
        canon_type(&h.return_type, types, &mut HashSet::new())
    );
    if !h.given.is_empty() {
        let mut given = h.given.clone();
        given.sort();
        let _ = write!(out, " given {}", given.join(", "));
    }
    out
}

/// P8.1 (#1512, [DECISION C]): the canonical form of a [`CapabilityOpSignature`]
/// — added by PR #1517's own bot review (finding #3): a `capability`
/// declaration's own ops are the abstract signature a consumer's
/// `Cap.op(...)` call site compiles against.
pub fn canon_capability_op_signature(
    op: &CapabilityOpSignature,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> String {
    let mut out = String::new();
    let _ = write!(out, "fn {}", op.name);
    if !op.type_params.is_empty() {
        let _ = write!(out, "[{}]", op.type_params.join(", "));
    }
    let _ = write!(out, "({})", canon_params(&op.params, types));
    let _ = write!(
        out,
        " -> {}",
        canon_type(&op.return_type, types, &mut HashSet::new())
    );
    out
}

/// P8.1 (#1512, [DECISION C]): the canonical form of a [`ProtocolSignature`]
/// — added by PR #1517's own bot review ("worth a look"): `from http` vs.
/// `from queue(...)` vs. `from websocket(...)` changes a service's entire
/// external surface. `Events`' own `pattern`/`schema_dispatch` render only as
/// presence (see `unit_signature.rs`'s own module doc comment for why).
fn canon_protocol_signature(
    p: &ProtocolSignature,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> String {
    match p {
        ProtocolSignature::Call => "call".to_string(),
        ProtocolSignature::Http => "http".to_string(),
        ProtocolSignature::Cron => "cron".to_string(),
        ProtocolSignature::Queue { name } => format!("queue {name:?}"),
        ProtocolSignature::WebSocket { in_type, out_type } => format!(
            "websocket in={} out={}",
            canon_type(in_type, types, &mut HashSet::new()),
            canon_type(out_type, types, &mut HashSet::new())
        ),
        ProtocolSignature::Events {
            event_type,
            has_pattern,
            has_schema_dispatch,
        } => format!(
            "events {} pattern={has_pattern} schema_dispatch={has_schema_dispatch}",
            canon_type(event_type, types, &mut HashSet::new())
        ),
    }
}

/// P8.1 (#1512, [DECISION C]): the canonical form of a [`StoreFieldSignature`].
pub fn canon_store_field_signature(
    f: &StoreFieldSignature,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> String {
    let mut out = String::new();
    let _ = write!(out, "{}: {}", f.name, f.kind_head);
    if !f.kind_args.is_empty() {
        let args: Vec<String> = f
            .kind_args
            .iter()
            .map(|a| canon_type(a, types, &mut HashSet::new()))
            .collect();
        let _ = write!(out, "[{}]", args.join(", "));
    }
    out
}

/// P8.1 (#1512, [DECISION C]): the canonical form of a unit's own combined
/// type table — the cross-context-types category, `combined_types_for`'s
/// output reused unchanged. Each named type expands structurally via
/// [`canon_named_in`] (through a synthetic, zero-span [`TypeRef::Named`] —
/// spans are erased by every canonical renderer in this module already, so a
/// dummy span here carries no information loss), sorted by name so map
/// iteration order can't perturb the rendering.
fn canon_types_table(types: &HashMap<String, Arc<TypeDecl>>) -> String {
    let mut names: Vec<&String> = types.keys().collect();
    names.sort();
    let mut out = String::new();
    for name in names {
        let named = TypeRef::Named(Ident {
            name: name.clone(),
            span: Span::new(0, 0),
        });
        let _ = write!(
            out,
            "{}={};",
            name,
            canon_type(&named, types, &mut HashSet::new())
        );
    }
    out
}

/// P8.1 (#1512, [DECISION C]): [`UnitSignature::canonical`]'s own
/// implementation — the single rendering R3.14's own stability proof
/// compares. Every category is rendered sorted (by the caller's own
/// `BTreeMap`/`BTreeSet` keys, or an explicit sort for `Vec`-shaped fields
/// like a handler's own `given`), so construction order never perturbs the
/// form — the same "compare the canonical string, not map/vec iteration
/// order" discipline [`service_normal_form`]'s record-field sort already
/// established for ADR 0200.
pub fn canon_unit_signature(sig: &UnitSignature) -> String {
    let types = &sig.combined_types;
    let mut out = String::new();
    let _ = writeln!(out, "unit {}", sig.id.0);
    let _ = writeln!(out, "types: {}", canon_types_table(types));

    out.push_str("fns:\n");
    for f in sig.fns.values() {
        let _ = writeln!(out, "  {}", canon_fn_signature(f, types));
    }

    // Methods (PR #1517's own bot review, finding #2): rendered per owning
    // type, instance and static kept in their own sections so a rename
    // between the two buckets is visible too, not just a param/return change
    // within one.
    out.push_str("methods:\n");
    for (owner, mt) in &sig.methods {
        let MethodTableSignature { instance, statics } = mt;
        for f in instance.values() {
            let _ = writeln!(out, "  {owner}.{}", canon_fn_signature(f, types));
        }
        for f in statics.values() {
            let _ = writeln!(out, "  {owner}::{}", canon_fn_signature(f, types));
        }
    }

    out.push_str("service_handlers:\n");
    for (owner, hs) in &sig.service_handlers {
        for h in hs {
            let _ = writeln!(out, "  {owner}: {}", canon_handler_signature(h, types));
        }
    }

    out.push_str("agent_handlers:\n");
    for (owner, hs) in &sig.agent_handlers {
        for h in hs {
            let _ = writeln!(out, "  {owner}: {}", canon_handler_signature(h, types));
        }
    }

    out.push_str("service_protocols:\n");
    for (owner, p) in &sig.service_protocols {
        let _ = writeln!(out, "  {owner}: {}", canon_protocol_signature(p, types));
    }

    out.push_str("store:\n");
    for (owner, fs) in &sig.store_fields {
        for f in fs {
            let _ = writeln!(out, "  {owner}: {}", canon_store_field_signature(f, types));
        }
    }

    out.push_str("capabilities:\n");
    for c in &sig.capabilities.exported {
        let _ = writeln!(out, "  exports {c}");
    }
    for (cap, ops) in &sig.capabilities.declared {
        for op in ops {
            let _ = writeln!(
                out,
                "  declares {cap}.{}",
                canon_capability_op_signature(op, types)
            );
        }
    }
    for (cap, given) in &sig.capabilities.provider_given {
        let mut given = given.clone();
        given.sort();
        let _ = writeln!(out, "  provides {cap} given {}", given.join(", "));
    }
    for (svc, given) in &sig.capabilities.service_given {
        let mut given = given.clone();
        given.sort();
        let _ = writeln!(out, "  service {svc} given {}", given.join(", "));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use bynk_syntax::ast::RefinementPred;

    #[test]
    fn predicate_order_does_not_change_the_form() {
        // The precondition the whole increment rests on: `String where NonEmpty,
        // MaxLen(10)` and the same predicates reordered are the *same type*, so
        // they must canonicalise — and therefore hash — identically. Hashing
        // source order would 409 two contexts that agree perfectly.
        let a = Refinement {
            predicates: vec![
                RefinementPred {
                    kind: PredKind::NonEmpty,
                    span: sp(),
                },
                RefinementPred {
                    kind: PredKind::MaxLength(10),
                    span: sp(),
                },
            ],
            span: sp(),
        };
        let b = Refinement {
            predicates: vec![
                RefinementPred {
                    kind: PredKind::MaxLength(10),
                    span: sp(),
                },
                RefinementPred {
                    kind: PredKind::NonEmpty,
                    span: sp(),
                },
            ],
            span: sp(),
        };
        assert_eq!(canon_refinement(Some(&a)), canon_refinement(Some(&b)));
        assert_eq!(
            contract_hash(&canon_refinement(Some(&a))),
            contract_hash(&canon_refinement(Some(&b)))
        );
    }

    #[test]
    fn a_different_predicate_set_changes_the_form() {
        let a = Refinement {
            predicates: vec![RefinementPred {
                kind: PredKind::MaxLength(10),
                span: sp(),
            }],
            span: sp(),
        };
        let b = Refinement {
            predicates: vec![RefinementPred {
                kind: PredKind::MaxLength(11),
                span: sp(),
            }],
            span: sp(),
        };
        assert_ne!(canon_refinement(Some(&a)), canon_refinement(Some(&b)));
    }

    #[test]
    fn fnv1a_matches_the_published_vectors() {
        // FNV-1a 64-bit reference vectors. The point of hand-rolling a
        // *specified* hash is that two compilers agree forever; pin it.
        assert_eq!(contract_hash(""), "cbf29ce484222325");
        assert_eq!(contract_hash("a"), "af63dc4c8601ec8c");
        assert_eq!(contract_hash("foobar"), "85944171f73967e8");
    }

    /// Build a type table by parsing a `commons`, so these tests exercise real
    /// declarations rather than hand-assembled AST.
    fn types_of(src: &str) -> HashMap<String, Arc<TypeDecl>> {
        let tokens = bynk_syntax::lexer::tokenize(src).expect("lex");
        let commons = bynk_syntax::parser::parse(&tokens, src).expect("parse");
        commons
            .items
            .iter()
            .filter_map(|i| match i {
                bynk_syntax::ast::CommonsItem::Type(t) => {
                    Some((t.name.name.clone(), Arc::new(t.clone())))
                }
                _ => None,
            })
            .collect()
    }

    fn named(n: &str) -> TypeRef {
        TypeRef::Named(bynk_syntax::ast::Ident {
            name: n.to_string(),
            span: sp(),
        })
    }

    fn svc(param_ty: TypeRef) -> CrossContextService {
        CrossContextService {
            name: "probe".to_string(),
            params: vec![("p".to_string(), param_ty)],
            return_type: TypeRef::Base(bynk_syntax::ast::BaseType::String, sp()),
            span: sp(),
            foreign_callers: Vec::new(),
        }
    }

    /// A record's **field order** is not wire-observable — a JSON object is
    /// unordered — so it must not move the hash. This is the false-positive side,
    /// and it is the one that matters most: a spurious 409 breaks a working
    /// deployment.
    #[test]
    fn record_field_order_does_not_change_the_hash() {
        let a = types_of("commons x\n\ntype P = { one: Int, two: String }\n");
        let b = types_of("commons x\n\ntype P = { two: String, one: Int }\n");
        assert_eq!(
            service_contract_hash(&svc(named("P")), &a),
            service_contract_hash(&svc(named("P")), &b),
        );
    }

    /// Field **presence** and **type**, by contrast, are exactly what the hash
    /// exists to pin. These are the cases a co-compiled build rejects
    /// structurally — but a skewed *deploy* cannot, which is why the hash carries
    /// them.
    #[test]
    fn field_presence_name_and_type_change_the_hash() {
        let base = types_of("commons x\n\ntype P = { one: Int, two: String }\n");
        let h = service_contract_hash(&svc(named("P")), &base);

        let renamed = types_of("commons x\n\ntype P = { one: Int, three: String }\n");
        assert_ne!(
            h,
            service_contract_hash(&svc(named("P")), &renamed),
            "rename"
        );

        let dropped = types_of("commons x\n\ntype P = { two: String }\n");
        assert_ne!(h, service_contract_hash(&svc(named("P")), &dropped), "drop");

        let retyped = types_of("commons x\n\ntype P = { one: String, two: String }\n");
        assert_ne!(
            h,
            service_contract_hash(&svc(named("P")), &retyped),
            "retype"
        );
    }

    /// #1969: an inline field refinement is part of the field's type, and the
    /// decoder checks it, so adding, changing or dropping one changes the
    /// hash. Its predicates canonicalise as a set, like a named refined type's.
    #[test]
    fn an_inline_field_refinement_changes_the_hash_as_a_set() {
        let plain = types_of("commons x\n\ntype P = { qty: Int }\n");
        let refined = types_of("commons x\n\ntype P = { qty: Int where Positive }\n");
        let both =
            types_of("commons x\n\ntype P = { name: String where NonEmpty && MaxLength(4) }\n");
        let swapped =
            types_of("commons x\n\ntype P = { name: String where MaxLength(4) && NonEmpty }\n");
        assert_ne!(
            service_contract_hash(&svc(named("P")), &plain),
            service_contract_hash(&svc(named("P")), &refined),
        );
        assert_eq!(
            service_normal_form(&svc(named("P")), &refined),
            "probe(p: P{qty: Int where Positive}) -> String"
        );
        assert_eq!(
            service_contract_hash(&svc(named("P")), &both),
            service_contract_hash(&svc(named("P")), &swapped),
        );
    }

    /// A sum's **variant order** is invisible to the wire (the payload carries a
    /// `kind` discriminant); its variant *set* is not.
    #[test]
    fn sum_variant_order_does_not_change_the_hash_but_the_set_does() {
        let a = types_of("commons x\n\ntype E = enum { Alpha, Beta }\n");
        let b = types_of("commons x\n\ntype E = enum { Beta, Alpha }\n");
        assert_eq!(
            service_contract_hash(&svc(named("E")), &a),
            service_contract_hash(&svc(named("E")), &b),
        );
        let c = types_of("commons x\n\ntype E = enum { Alpha, Gamma }\n");
        assert_ne!(
            service_contract_hash(&svc(named("E")), &a),
            service_contract_hash(&svc(named("E")), &c),
        );
    }

    /// An **opaque** type's predicate is excluded: the consumer cannot see it by
    /// construction, so no consumer behaviour can depend on it, and including it
    /// would manufacture skew between two contexts that cannot disagree. Its
    /// *representation* is still part of the contract.
    #[test]
    fn an_opaque_types_predicate_is_excluded_but_its_representation_is_not() {
        let loose = types_of("commons x\n\ntype Id = opaque String where NonEmpty\n");
        let tight = types_of("commons x\n\ntype Id = opaque String where MaxLength(4)\n");
        assert_eq!(
            service_contract_hash(&svc(named("Id")), &loose),
            service_contract_hash(&svc(named("Id")), &tight),
            "tightening an opaque predicate must not 409 a caller that cannot see it"
        );

        // A *transparent* refined type is the opposite: the consumer can see the
        // predicate, so it is part of the contract.
        let ra = types_of("commons x\n\ntype C = String where MaxLength(4)\n");
        let rb = types_of("commons x\n\ntype C = String where MaxLength(5)\n");
        assert_ne!(
            service_contract_hash(&svc(named("C")), &ra),
            service_contract_hash(&svc(named("C")), &rb),
        );
    }

    /// `NonEmpty` is sugar for `MinLength(1)` (R12.2, T1.8, Decision A) —
    /// pinned directly against the canonicaliser so a future edit to this arm
    /// is a deliberate, visible change rather than a silent regression.
    #[test]
    fn non_empty_canonicalises_to_min_length_one() {
        assert_eq!(canon_predicate(&PredKind::NonEmpty), "MinLength(1)");
    }

    /// The counterpart to `non_empty_canonicalises_to_min_length_one`: the
    /// `Positive`/`NonNegative` → `InRange` fold is **declined** (#1049) —
    /// neither base has a writable bound standing for `∞`. Pinned so
    /// reversing that decision trips a named test rather than a fixture
    /// hash.
    #[test]
    fn positive_and_non_negative_stay_their_own_canonical_literals() {
        assert_eq!(canon_predicate(&PredKind::Positive), "Positive");
        assert_eq!(canon_predicate(&PredKind::NonNegative), "NonNegative");
    }

    /// The consequence that matters: two boundary types spelling the same
    /// refinement differently must hash identically, or two contexts that
    /// agree perfectly 409 each other.
    #[test]
    fn non_empty_and_min_length_one_hash_identically() {
        let a = types_of("commons x\n\ntype C = String where NonEmpty\n");
        let b = types_of("commons x\n\ntype C = String where MinLength(1)\n");
        assert_eq!(
            service_contract_hash(&svc(named("C")), &a),
            service_contract_hash(&svc(named("C")), &b),
            "NonEmpty and MinLength(1) are the same refinement and must hash the same"
        );
    }

    /// After the fold, `NonEmpty` and `MinLength(1)` are the same canonical
    /// string, so a redundant `NonEmpty && MinLength(1)` conjunction must dedup
    /// to one entry, not two — the same idempotence `canon_refinement`'s sort+
    /// dedup already promises for any other repeated predicate.
    #[test]
    fn non_empty_and_min_length_one_together_dedup_to_one_entry() {
        let r = Refinement {
            predicates: vec![
                RefinementPred {
                    kind: PredKind::NonEmpty,
                    span: sp(),
                },
                RefinementPred {
                    kind: PredKind::MinLength(1),
                    span: sp(),
                },
            ],
            span: sp(),
        };
        assert_eq!(canon_refinement(Some(&r)), "where MinLength(1)");
    }

    /// A recursive record is a legal contract (its codec terminates on the data),
    /// but its expansion is not — the walk must terminate rather than blow the
    /// stack, and two different recursive shapes must still differ.
    #[test]
    fn a_recursive_record_terminates_and_stays_distinguishable() {
        let a = types_of("commons x\n\ntype Node = { v: Int, next: Option[Node] }\n");
        let b = types_of("commons x\n\ntype Node = { v: String, next: Option[Node] }\n");
        let ha = service_contract_hash(&svc(named("Node")), &a);
        assert_ne!(ha, service_contract_hash(&svc(named("Node")), &b));
    }

    /// State-migrations S0 (#2204): the storage renderings are structural —
    /// stored bytes carry no type names — and recursion still terminates,
    /// with the back-reference as the only name left.
    #[test]
    fn storage_renderings_drop_type_names_and_terminate_on_recursion() {
        let t =
            types_of("commons x\n\ntype Node = { v: Int where NonNegative, next: Option[Node] }\n");
        let (structure, full) = canon_storage_type(&named("Node"), &t);
        assert_eq!(structure, "{next: Option[@Node], v: Int}");
        assert_eq!(full, "{next: Option[@Node], v: Int where NonNegative}");

        let renamed =
            types_of("commons x\n\ntype Link = { v: Int where NonNegative, next: Option[Link] }\n");
        let (renamed_structure, _) = canon_storage_type(&named("Link"), &renamed);
        assert_eq!(renamed_structure, "{next: Option[@Link], v: Int}");

        let flat_a = types_of("commons x\n\ntype A = { title: String }\n");
        let flat_b = types_of("commons x\n\ntype B = { title: String }\n");
        assert_eq!(
            canon_storage_type(&named("A"), &flat_a),
            canon_storage_type(&named("B"), &flat_b),
            "renaming a non-recursive type changes nothing on disk"
        );

        // Nor does renaming a refined type, or one used as a map key or as a
        // sum variant's payload: no position in the walk is nominal.
        let qty = types_of("commons x\n\ntype Qty = Int where NonNegative\n");
        let count = types_of("commons x\n\ntype Count = Int where NonNegative\n");
        assert_eq!(
            canon_storage_type(&named("Qty"), &qty),
            canon_storage_type(&named("Count"), &count),
        );
        let map = |key: &str| TypeRef::Map(Box::new(named(key)), Box::new(named(key)), sp());
        assert_eq!(
            canon_storage_type(&map("Qty"), &qty),
            canon_storage_type(&map("Count"), &count),
        );
        let sum_a = types_of("commons x\n\ntype P = { n: Int }\ntype S = | One(p: P) | Two\n");
        let sum_b = types_of("commons x\n\ntype Q = { n: Int }\ntype S = | One(p: Q) | Two\n");
        assert_eq!(
            canon_storage_type(&named("S"), &sum_a),
            canon_storage_type(&named("S"), &sum_b),
        );
    }

    /// The stored owner re-validates its opaque values, so the full storage
    /// rendering keeps an opaque predicate the contract form elides — and the
    /// contract hash itself is unchanged by the new modes.
    #[test]
    fn storage_full_rendering_keeps_an_opaque_predicate_the_contract_elides() {
        let a = types_of("commons x\n\ntype Code = opaque String where MaxLength(10)\n");
        let b = types_of("commons x\n\ntype Code = opaque String where MaxLength(5)\n");
        let (sa, fa) = canon_storage_type(&named("Code"), &a);
        let (sb, fb) = canon_storage_type(&named("Code"), &b);
        assert_eq!(sa, "String");
        assert_eq!(sa, sb, "the structure ignores the predicate");
        assert_ne!(fa, fb, "the full form sees the tightened predicate");
        assert_eq!(
            service_contract_hash(&svc(named("Code")), &a),
            service_contract_hash(&svc(named("Code")), &b),
            "the contract still elides an opaque predicate"
        );
    }

    /// A generic type's **parameter name** is not wire-observable: `Page[T]` and
    /// the same declaration spelled with `U` are the same type with the same
    /// shape. Renaming one is a refactor, and must not 409 a caller.
    ///
    /// Caught in review of #658: the body used to expand with the parameter
    /// *unsubstituted*, so the name leaked into the form. Same class as record
    /// field order and predicate order — the false-positive side the module's
    /// standard exists to protect.
    #[test]
    fn a_generic_type_parameter_rename_does_not_change_the_hash() {
        let t = types_of(
            "commons x\n\ntype Order = { id: Int }\ntype Page[T] = { items: List[T], total: Int }\n",
        );
        let u = types_of(
            "commons x\n\ntype Order = { id: Int }\ntype Page[U] = { items: List[U], total: Int }\n",
        );
        let app = TypeRef::App {
            name: bynk_syntax::ast::Ident {
                name: "Page".to_string(),
                span: sp(),
            },
            args: vec![named("Order")],
            span: sp(),
        };
        assert_eq!(
            service_contract_hash(&svc(app.clone()), &t),
            service_contract_hash(&svc(app), &u),
            "renaming a generic type parameter must not change the contract hash"
        );
    }

    /// The converse: the *argument* a generic is instantiated at is entirely
    /// wire-observable, so it must still move the hash.
    #[test]
    fn a_generic_argument_change_does_change_the_hash() {
        let t = types_of(
            "commons x\n\ntype Order = { id: Int }\ntype Other = { id: String }\ntype Page[T] = { items: List[T] }\n",
        );
        let app = |arg: &str| TypeRef::App {
            name: bynk_syntax::ast::Ident {
                name: "Page".to_string(),
                span: sp(),
            },
            args: vec![named(arg)],
            span: sp(),
        };
        assert_ne!(
            service_contract_hash(&svc(app("Order")), &t),
            service_contract_hash(&svc(app("Other")), &t),
        );
    }

    /// And the parameter is genuinely *substituted*, not merely ignored: the form
    /// shows the instantiated shape rather than a dangling `T`.
    #[test]
    fn a_generic_body_expands_over_its_concrete_argument() {
        let t = types_of("commons x\n\ntype Page[T] = { items: List[T] }\n");
        let app = TypeRef::App {
            name: bynk_syntax::ast::Ident {
                name: "Page".to_string(),
                span: sp(),
            },
            args: vec![TypeRef::Base(bynk_syntax::ast::BaseType::Int, sp())],
            span: sp(),
        };
        let nf = service_normal_form(&svc(app), &t);
        assert!(nf.contains("List[Int]"), "{nf}");
        assert!(
            !nf.contains("List[T]"),
            "the parameter must not survive: {nf}"
        );
    }

    fn sp() -> bynk_syntax::span::Span {
        bynk_syntax::span::Span::new(0, 0)
    }
}
