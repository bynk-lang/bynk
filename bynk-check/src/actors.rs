//! v0.45 actor contracts (the actors-foundations slice).
//!
//! An `actor` declaration is a nominal *boundary contract* (ADR Q1): a closed,
//! compiler-known authentication `Scheme` plus an optional sealed identity. A
//! handler consumes an actor on its `by` clause; the boundary verifies the
//! scheme and mints the identity before the body runs (two-phase, fail-closed —
//! ADR Q5/Q2).
//!
//! This module holds the compiler-known parts: the closed scheme set, the
//! prelude actors, the per-protocol default actors, and the admissible-scheme
//! sets. Foundations admits only the two zero-crypto schemes (`None`,
//! `Internal`); `Bearer`/`Signature` are reserved-and-rejected.

use std::collections::{BTreeSet, HashMap};

use bynk_syntax::ast::{
    BinOp, ByClause, Expr, ExprKind, Handler, HandlerKind, ServiceProtocol, TypeRef, UnaryOp,
};
use bynk_syntax::span::Span;

/// P6.49 (design/tracks/the-ir.md §6b), following P6.27's `ExprId` precedent:
/// every reader in this module — [`bearer_seam_for`], [`oidc_seam_for`],
/// [`signature_seam_for`], [`sum_members_for`], [`caller_binder_for`] — is
/// already parameterised by `&HashMap<String, ActorDecl>`. `bynk-emit` clones
/// and forwards this table (`project.rs`'s `EmitProjectCtx::actors`,
/// `emitter/workers.rs`/`workers_entry.rs`'s own `actors` parameters) without
/// ever reading an `ActorDecl` field itself — P6.34 investigated and declined
/// resolving it earlier, since `bynk-emit`'s own `lower_actor_seam_ir` needs
/// the raw declarations to do its own resolution; this re-export changes the
/// import path, not that data flow.
pub use bynk_syntax::ast::ActorDecl;

/// The authentication scheme — a closed, compiler-known set (ADR Q1). Sealed
/// now, openable later by widening this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    /// Anonymous — no verification; identity is `()`. (`Visitor`.)
    None,
    /// In-system / platform trust — the channel itself is the assertion
    /// (service-binding / platform dispatch). Admitted in Foundations.
    Internal,
    /// Bearer token — compiler-generated JWT/HS256 verification (ADR 0085).
    Bearer,
    /// Request signature — HMAC-SHA256 over the body (ADR 0089).
    Signature,
    /// OIDC/JWKS — compiler-generated verification of an asymmetrically-signed
    /// (RS256/ES256) JWT against a provider's published JWKS, checking
    /// `iss`/`aud`/`exp`/`nbf` and minting the identity from `sub` (ADR 0175).
    /// The first scheme opened via the "user-configured verifier" route: the
    /// trust root is the provider's public key set (a URL), so the declaration
    /// carries **no inline secret** — only public trust parameters.
    Oidc,
    /// I3 (#1971, interop track D2): a caller that is not Bynk, declared by the
    /// context it calls (`auth = Foreign(secret = "LEGACY_CALL_KEY")`). It signs
    /// a call like a Bynk caller does (`bynk_check::wire_format`), naming
    /// itself by the actor's name, under the declared secret. Admitted on
    /// `call` only, and resolves to its own actor, never to `Caller`.
    Foreign,
}

impl Scheme {
    /// Classify a scheme name written in `auth = <Scheme>`. `None` means the
    /// name is not one of the compiler-known schemes.
    pub fn from_name(s: &str) -> Option<Scheme> {
        Some(match s {
            "None" => Scheme::None,
            "Internal" => Scheme::Internal,
            "Bearer" => Scheme::Bearer,
            "Signature" => Scheme::Signature,
            "Oidc" => Scheme::Oidc,
            "Foreign" => Scheme::Foreign,
            _ => return None,
        })
    }

    /// The schemes the compiler can emit verification for. v0.45 admitted the
    /// two zero-crypto schemes (`None`/`Internal`); v0.47 added `Bearer`
    /// (JWT/HS256); v0.51 added `Signature` (HMAC over the body); v0.151 adds
    /// `Oidc` (JWKS/RS256+ES256). All five schemes are now admitted.
    pub fn admitted(self) -> bool {
        matches!(
            self,
            Scheme::None
                | Scheme::Internal
                | Scheme::Bearer
                | Scheme::Signature
                | Scheme::Oidc
                | Scheme::Foreign
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Scheme::None => "None",
            Scheme::Internal => "Internal",
            Scheme::Bearer => "Bearer",
            Scheme::Signature => "Signature",
            Scheme::Oidc => "Oidc",
            Scheme::Foreign => "Foreign",
        }
    }
}

/// The identity a verified actor yields (ADR Q2). In Foundations this is `()`
/// for trivial actors, the built-in sealed `CallerId` for the cross-context
/// `Internal` channel (Q7, folded in), or a context-owned declared type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    /// `()` — `None` actors and platform-tag `Internal` actors.
    Unit,
    /// The built-in sealed calling-context identity (Q7). Minted at the
    /// service-binding seam; read-only and never re-checked.
    CallerId,
    /// A context-owned declared type named in `identity = <T>`.
    Declared(String),
}

/// The built-in sealed identity type for the cross-context calling principal.
pub const CALLER_ID: &str = "CallerId";

/// A resolved actor contract: its scheme and the identity it yields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contract {
    pub scheme: Scheme,
    pub identity: Identity,
}

/// The prelude actors — compiler-known boundary contracts available without a
/// declaration. They back the per-protocol defaults and let public HTTP routes
/// write `by v: Visitor` without ceremony.
pub fn prelude_actor(name: &str) -> Option<Contract> {
    Some(match name {
        // Anonymous public surface — the only safe HTTP actor in Foundations.
        "Visitor" => Contract {
            scheme: Scheme::None,
            identity: Identity::Unit,
        },
        // Platform schedulers / producers — Internal, carrying no useful
        // identity payload (a bare tag).
        "Scheduler" | "Producer" => Contract {
            scheme: Scheme::Internal,
            identity: Identity::Unit,
        },
        // The cross-context calling principal — Internal, yielding the sealed
        // `CallerId` (Q7).
        "Caller" => Contract {
            scheme: Scheme::Internal,
            identity: Identity::CallerId,
        },
        _ => return None,
    })
}

/// The default actor a handler inherits when it omits `by`, by protocol (ADR
/// Q5). HTTP has no safe default — `by` is required there.
pub fn default_actor(protocol: &ServiceProtocol) -> Option<&'static str> {
    match protocol {
        ServiceProtocol::Call => Some("Caller"),
        ServiceProtocol::Cron => Some("Scheduler"),
        ServiceProtocol::Queue { .. } => Some("Producer"),
        // Events track, slice 0 (spine #936): delivery is runtime-triggered,
        // not an external caller — like `Queue`'s "Producer", the default
        // actor names the event's originating publisher.
        ServiceProtocol::Events { .. } => Some("Publisher"),
        // v0.103: like HTTP, a WebSocket upgrade has no safe default actor —
        // `by` is mandatory on `on open` (edge auth before accept, D-A).
        ServiceProtocol::Http | ServiceProtocol::WebSocket { .. } => None,
    }
}

/// v0.47: the data the emitter needs to lower a Bearer verification seam for a
/// handler — the `by` binder (v0.50: `None` for the binder-less verify-and-
/// discard form), the signing-secret env name, and the identity type to
/// construct from the JWT `sub` claim. Resolved only for a handler whose `by`
/// clause names a local Bearer actor; the checker guarantees the secret is
/// present and the identity is a string-constructible local type.
#[derive(Debug, Clone)]
pub struct BearerSeam {
    /// The identity binder, or `None` for `by <BearerActor>` (verify the token,
    /// don't capture the identity). When `None` the seam still verifies fail-
    /// closed but mints no identity and threads nothing into `deps`.
    pub binder: Option<String>,
    pub secret: String,
    pub identity_type: String,
    /// v0.53: the authorisation invariant when the `by` actor is a refinement
    /// (`actor Admin = User where <pred>`). The seam verifies the scheme (401),
    /// then checks this predicate against the verified claims (403 fail-closed),
    /// then mints the (base) identity. `None` for a plain Bearer actor.
    pub authorization: Option<ClaimPredicate>,
}

/// Resolve a handler's Bearer seam, if its `by` clause names a local Bearer
/// actor — or a **refinement** of one (v0.53), following the refinement to its
/// base for the scheme/secret/identity and carrying the authorisation
/// predicate. Returns `None` for non-Bearer handlers (prelude actors are never
/// Bearer) — those emit unchanged.
/// #706: whether a `by` clause names a **Bearer**-secured actor (following a
/// refinement to its base). The routes for which `by Nobody` can drive the auth
/// seam to a `401` — an unsecured (`Visitor`/`None`) or `Signature`/`Oidc` route
/// has no Bearer seam the test driver knows how to leave unauthenticated, so
/// `by Nobody` there is rejected (`bynk.test.nobody_needs_secured_route`). The
/// scheme check mirrors [`bearer_seam_for`].
pub fn by_clause_is_bearer(by: &ByClause, actors: &HashMap<String, ActorDecl>) -> bool {
    let Some(named) = actors.get(&by.primary().name) else {
        return false;
    };
    let base = match &named.refinement {
        Some(r) => match actors.get(&r.base.name) {
            Some(b) => b,
            None => return false,
        },
        None => named,
    };
    base.auth
        .as_ref()
        .and_then(|a| Scheme::from_name(a.name.as_str()))
        == Some(Scheme::Bearer)
}

pub fn bearer_seam_for(
    handler: &Handler,
    actors: &HashMap<String, ActorDecl>,
) -> Option<BearerSeam> {
    let by = handler.by_clause.as_ref()?;
    let named = actors.get(&by.primary().name)?;
    // Follow a refinement to its base; carry the authorisation predicate. The
    // checker guarantees a refinement's base is Bearer and its predicate parses.
    let (base, authorization) = match &named.refinement {
        Some(r) => (
            actors.get(&r.base.name)?,
            parse_claim_predicate(&r.predicate).ok(),
        ),
        None => (named, None),
    };
    if Scheme::from_name(base.auth.as_ref()?.name.as_str()) != Some(Scheme::Bearer) {
        return None;
    }
    let secret = base.scheme_arg("secret")?.value.as_str()?.to_string();
    let TypeRef::Named(id) = base.identity.as_ref()? else {
        return None;
    };
    Some(BearerSeam {
        binder: by.binder.as_ref().map(|b| b.name.clone()),
        secret,
        identity_type: id.name.clone(),
        authorization,
    })
}

/// v0.151: the data the emitter needs to lower an OIDC/JWKS verification seam —
/// the `by` binder (or `None` for the verify-and-discard form), the public
/// trust parameters (`issuer`, `audience`, `jwks` URL), and the identity type
/// to construct from the verified `sub` claim. Resolved only for a handler
/// whose `by` clause names a local `Oidc` actor. Unlike Bearer/Signature, the
/// seam carries **no secret env name** — the trust root is the provider's
/// published public key set (`jwks`).
#[derive(Debug, Clone)]
pub struct OidcSeam {
    /// The identity binder, or `None` for `by <OidcActor>` (verify, don't
    /// capture the identity). When `None` the seam still verifies fail-closed
    /// but mints no identity and threads nothing into `deps`.
    pub binder: Option<String>,
    /// The expected `iss` claim (and the provider whose JWKS anchors trust).
    pub issuer: String,
    /// The expected `aud` claim (this API's audience identifier).
    pub audience: String,
    /// The JWKS endpoint URL the verifier fetches signing keys from.
    pub jwks: String,
    /// The context-owned, string-constructible identity type minted from `sub`.
    pub identity_type: String,
    /// #2380 (L5): the environment variable whose JSON, when set, overrides
    /// `issuer`/`audience`/`jwks` at run time.
    pub trust_from: Option<String>,
}

/// Resolve a handler's OIDC seam, if its `by` clause names a single local
/// `Oidc` actor. Returns `None` for non-Oidc handlers, for a multi-actor `by`
/// clause (Oidc is single-actor this slice), and for a refinement (refinement
/// over Oidc is a later slice) — those follow the existing seam paths. The
/// checker guarantees `issuer`/`audience`/`jwks` are present and the identity
/// is a string-constructible local type.
pub fn oidc_seam_for(handler: &Handler, actors: &HashMap<String, ActorDecl>) -> Option<OidcSeam> {
    let by = handler.by_clause.as_ref()?;
    if by.is_sum() {
        return None;
    }
    let actor = actors.get(&by.primary().name)?;
    // A refinement's `auth` is `None`; it falls through here and follows the
    // Bearer refinement path (or is rejected). An Oidc base is not narrowed.
    if Scheme::from_name(actor.auth.as_ref()?.name.as_str()) != Some(Scheme::Oidc) {
        return None;
    }
    let issuer = actor.scheme_arg("issuer")?.value.as_str()?.to_string();
    let audience = actor.scheme_arg("audience")?.value.as_str()?.to_string();
    let jwks = actor.scheme_arg("jwks")?.value.as_str()?.to_string();
    let TypeRef::Named(id) = actor.identity.as_ref()? else {
        return None;
    };
    Some(OidcSeam {
        binder: by.binder.as_ref().map(|b| b.name.clone()),
        issuer,
        audience,
        jwks,
        identity_type: id.name.clone(),
        trust_from: actor
            .scheme_arg("trustFrom")
            .and_then(|a| a.value.as_str())
            .map(str::to_string),
    })
}

/// v0.54: the binder of a cross-context `on call … by c: Caller` handler that
/// captures a live `CallerId` (the calling context's name, Q7). `None` unless
/// the handler binds an identity whose contract is `CallerId` — i.e. the
/// `Caller` prelude actor (the only source of `CallerId`). A binder-less
/// `on call` (or one inheriting the `Caller` default) captures nothing and is
/// unaffected.
pub fn caller_binder_for(handler: &Handler, actors: &HashMap<String, ActorDecl>) -> Option<String> {
    // `CallerId` is a cross-context `on call` concept; the checker rejects a
    // `Caller` actor on other protocols (`scheme_not_admissible`), but guard here
    // too so the caller seam is never emitted off the call path.
    if !matches!(handler.kind, HandlerKind::Call) {
        return None;
    }
    let by = handler.by_clause.as_ref()?;
    // I3 (#1971): a sum (`by who: Caller | Legacy`) binds the resolved actor,
    // not a `CallerId`, even when `Caller` is its first member
    // ([`call_admission_for`]).
    if by.is_sum() {
        return None;
    }
    let binder = by.binder.as_ref()?;
    let name = &by.primary().name;
    // `CallerId` is yielded only by the `Caller` prelude actor; a local actor
    // never declares it. A binder that collides with a param is suppressed
    // upstream, mirroring the other seams.
    let is_caller = !actors.contains_key(name)
        && prelude_actor(name).map(|c| c.identity) == Some(Identity::CallerId)
        && !handler.params.iter().any(|p| p.name.name == binder.name);
    is_caller.then(|| binder.name.clone())
}

/// I3 (#1971): a declared foreign caller, as a `call` handler admits it: the
/// actor's name, which the caller sends as `X-Bynk-Caller`, and the secret
/// the callee verifies its signature under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignCaller {
    pub actor: String,
    pub secret: String,
    /// The foreign Worker's name, when the declaration gives one
    /// (`worker = "ledger-svc"`), so `bynk deploy`'s hand-off can name it.
    pub worker: Option<String>,
}

/// I3 (#1971): the foreign caller an actor declaration describes, if it is a
/// `Foreign`-scheme actor with a `secret`.
pub fn foreign_caller_of(decl: &ActorDecl) -> Option<ForeignCaller> {
    (Scheme::from_name(decl.auth.as_ref()?.name.as_str())? == Scheme::Foreign).then_some(())?;
    Some(ForeignCaller {
        actor: decl.name.name.clone(),
        secret: decl.scheme_arg("secret")?.value.as_str()?.to_string(),
        worker: decl
            .scheme_arg("worker")
            .and_then(|a| a.value.as_str())
            .map(str::to_string),
    })
}

/// I3 (#1971): which verified callers an `on call` handler admits. Every call
/// is authenticated before it reaches a handler (`wire_format`); this is what
/// the handler then admits, in its `by` clause's order. A Bynk context is
/// admitted by `Caller` (the default with no `by`) or a local `Internal`
/// actor; a foreign caller only by its own `Foreign` actor, never by `Caller`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallAdmission {
    /// Whether a Bynk context is admitted.
    pub contexts: bool,
    /// The foreign callers admitted, in `by` order.
    pub foreign: Vec<ForeignCaller>,
    /// For a sum, the members in `by` order: `None` for the member that
    /// admits a context (`Caller`), the foreign caller otherwise. The body's
    /// `match` reads the resolved one.
    pub sum: Option<Vec<CallSumMember>>,
}

/// I3 (#1971): one member of a `call` handler's actor sum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallSumMember {
    /// `Caller`: a Bynk context, with its name as the identity.
    Caller,
    /// A declared foreign caller, with no identity beyond its actor.
    Foreign(ForeignCaller),
}

impl CallSumMember {
    /// The member's tag, the actor's name.
    pub fn actor_name(&self) -> &str {
        match self {
            CallSumMember::Caller => "Caller",
            CallSumMember::Foreign(f) => &f.actor,
        }
    }
}

/// I3 (#1971): what an `on call` handler admits. `None` off the call path.
pub fn call_admission_for(
    handler: &Handler,
    actors: &HashMap<String, ActorDecl>,
) -> Option<CallAdmission> {
    if !matches!(handler.kind, HandlerKind::Call) {
        return None;
    }
    let Some(by) = handler.by_clause.as_ref() else {
        return Some(CallAdmission {
            contexts: true,
            foreign: Vec::new(),
            sum: None,
        });
    };
    let mut admission = CallAdmission {
        contexts: false,
        foreign: Vec::new(),
        sum: None,
    };
    let mut members = Vec::new();
    for actor in &by.actors {
        match actors.get(&actor.name) {
            // Classified by scheme, not by whether the declaration is whole:
            // a malformed `Foreign` actor (the checker reports it) admits
            // nobody, rather than falling through to admit contexts.
            Some(decl)
                if decl.auth.as_ref().and_then(|a| Scheme::from_name(&a.name))
                    == Some(Scheme::Foreign) =>
            {
                if let Some(f) = foreign_caller_of(decl) {
                    admission.foreign.push(f.clone());
                    members.push(CallSumMember::Foreign(f));
                }
            }
            Some(_) => {
                admission.contexts = true;
                members.push(CallSumMember::Caller);
            }
            None => {
                admission.contexts = true;
                members.push(CallSumMember::Caller);
            }
        }
    }
    if by.is_sum() {
        admission.sum = Some(members);
    }
    Some(admission)
}

/// v0.51: the data the emitter needs to lower a Signature verification seam —
/// the signing-secret env name, the signature header, and an optional
/// timestamp header + tolerance window for replay defence. Resolved only for a
/// handler whose `by` clause names a local Signature actor.
#[derive(Debug, Clone)]
pub struct SignatureSeam {
    pub secret: String,
    pub header: String,
    pub timestamp_header: Option<String>,
    pub tolerance_secs: Option<i64>,
}

/// Resolve a handler's Signature seam, if its `by` clause names a local
/// Signature actor. The checker guarantees `secret` and `header` are present.
pub fn signature_seam_for(
    handler: &Handler,
    actors: &HashMap<String, ActorDecl>,
) -> Option<SignatureSeam> {
    signature_seam_for_by(handler.by_clause.as_ref()?, actors)
}

/// #2155: the `Signature` seam a `by` clause's route is signed with at the
/// `system` tier — its one actor's, or an actor sum's first `Signature`
/// member's (what `by Forged`/`by Stale` sign as). `None` when it has none,
/// and for a sum with an open member ([`sum_has_open_member`]), whose seam
/// never refuses a bad signature.
pub fn signature_seam_of_route(
    by: &ByClause,
    actors: &HashMap<String, ActorDecl>,
) -> Option<SignatureSeam> {
    if !by.is_sum() {
        return signature_seam_for_by(by, actors);
    }
    if sum_has_open_member(by, actors) {
        return None;
    }
    by.actors.iter().find_map(|a| {
        let decl = actors.get(&a.name)?;
        (Scheme::from_name(decl.auth.as_ref()?.name.as_str()) == Some(Scheme::Signature))
            .then(|| signature_seam_from_decl(decl))
            .flatten()
    })
}

/// #2155: whether a `by` clause's route has a credential seam the `system`
/// driver can leave out (`by Nobody`): a Bearer actor (or a refinement of
/// one), a `Signature` actor, or an actor sum with such a member and no open
/// member ([`sum_has_open_member`]).
pub fn route_is_secured(by: &ByClause, actors: &HashMap<String, ActorDecl>) -> bool {
    if !by.is_sum() {
        return by_clause_is_bearer(by, actors) || signature_seam_for_by(by, actors).is_some();
    }
    if sum_has_open_member(by, actors) {
        return false;
    }
    by.actors.iter().any(|a| {
        actors
            .get(&a.name)
            .and_then(|d| d.auth.as_ref())
            .and_then(|auth| Scheme::from_name(auth.name.as_str()))
            .is_some_and(|s| matches!(s, Scheme::Bearer | Scheme::Signature))
    })
}

/// #2155: whether an actor sum has an *open* member — `Visitor`, or a
/// declared actor with no auth scheme. The sum's seam resolves a request with
/// a missing or bad credential as that member, so it never answers `401`:
/// `by Nobody`, `by Forged` and `by Stale` have nothing to drive there.
pub fn sum_has_open_member(by: &ByClause, actors: &HashMap<String, ActorDecl>) -> bool {
    by.actors.iter().any(|a| match actors.get(&a.name) {
        // A refinement narrows a secured base, so it is never open.
        Some(decl) if decl.refinement.is_some() => false,
        Some(decl) => decl
            .auth
            .as_ref()
            .and_then(|auth| Scheme::from_name(auth.name.as_str()))
            .is_none_or(|s| s == Scheme::None),
        None => prelude_actor(&a.name).is_some_and(|c| c.scheme == Scheme::None),
    })
}

/// #2135: [`signature_seam_for`] from the `by` clause alone — the seam of
/// its primary actor, when that is a local Signature actor.
pub fn signature_seam_for_by(
    by: &ByClause,
    actors: &HashMap<String, ActorDecl>,
) -> Option<SignatureSeam> {
    let actor = actors.get(&by.primary().name)?;
    if Scheme::from_name(actor.auth.as_ref()?.name.as_str()) != Some(Scheme::Signature) {
        return None;
    }
    signature_seam_from_decl(actor)
}

/// The Signature seam data carried by an actor declaration (its keyed config).
/// Shared by the single-actor `signature_seam_for` and the multi-actor
/// `sum_members_for`.
fn signature_seam_from_decl(actor: &ActorDecl) -> Option<SignatureSeam> {
    Some(SignatureSeam {
        secret: actor.scheme_arg("secret")?.value.as_str()?.to_string(),
        header: actor.scheme_arg("header")?.value.as_str()?.to_string(),
        timestamp_header: actor
            .scheme_arg("timestamp")
            .and_then(|a| a.value.as_str())
            .map(str::to_string),
        tolerance_secs: actor.scheme_arg("tolerance").and_then(|a| a.value.as_int()),
    })
}

/// v0.52: one resolved member of a multi-actor sum — the seam the emitter tries
/// at that position in the first-wins order. `actor_name` is the variant tag the
/// body matches on.
#[derive(Debug, Clone)]
pub struct SumMember {
    pub actor_name: String,
    pub seam: SumMemberSeam,
}

/// The verification a sum member contributes. `None` (a catch-all such as
/// `Visitor`) always resolves, so it terminates the order.
#[derive(Debug, Clone)]
pub enum SumMemberSeam {
    None,
    Bearer {
        secret: String,
        identity_type: String,
    },
    Signature(SignatureSeam),
}

impl SumMember {
    /// Whether resolving this member needs the raw request body read.
    pub fn needs_body(&self) -> bool {
        matches!(self.seam, SumMemberSeam::Signature(_))
    }
    /// The member's identity type name, if it mints one (Bearer). `None`/
    /// Signature members carry a unit identity.
    pub fn identity_type(&self) -> Option<&str> {
        match &self.seam {
            SumMemberSeam::Bearer { identity_type, .. } => Some(identity_type),
            _ => None,
        }
    }
}

/// v0.52: resolve a handler's `by` clause into ordered sum members, if it names
/// more than one actor. `None` for a single-actor handler (those keep the
/// existing seam paths). The checker has already validated peer/scheme/
/// reachability rules; this lowers the verified members for emission.
pub fn sum_members_for(
    handler: &Handler,
    actors: &HashMap<String, ActorDecl>,
) -> Option<Vec<SumMember>> {
    let by = handler.by_clause.as_ref()?;
    if !by.is_sum() {
        return None;
    }
    let mut members = Vec::new();
    for actor_ref in &by.actors {
        let seam = if let Some(decl) = actors.get(&actor_ref.name) {
            match Scheme::from_name(decl.auth.as_ref()?.name.as_str())? {
                Scheme::None => SumMemberSeam::None,
                Scheme::Bearer => {
                    let secret = decl.scheme_arg("secret")?.value.as_str()?.to_string();
                    let TypeRef::Named(id) = decl.identity.as_ref()? else {
                        return None;
                    };
                    SumMemberSeam::Bearer {
                        secret,
                        identity_type: id.name.clone(),
                    }
                }
                Scheme::Signature => SumMemberSeam::Signature(signature_seam_from_decl(decl)?),
                // v0.151: `Oidc` is single-actor only this slice — the checker
                // rejects it as a sum member (`bynk.actor.oidc_not_in_sum`), so
                // a well-formed program never reaches here with an Oidc peer.
                // Return `None` (fail closed → no sum emission) defensively.
                Scheme::Oidc => return None,
                // I3: a `Foreign` sum is a `call` sum (`call_admission_for`).
                Scheme::Internal | Scheme::Foreign => return None,
            }
        } else {
            // A prelude actor: only `Visitor` (scheme `None`) is an HTTP peer.
            match prelude_actor(&actor_ref.name) {
                Some(c) if c.scheme == Scheme::None => SumMemberSeam::None,
                _ => return None,
            }
        };
        members.push(SumMember {
            actor_name: actor_ref.name.clone(),
            seam,
        });
    }
    Some(members)
}

/// Whether `scheme` is admissible on `protocol` (the admissible-scheme-per-
/// protocol check). HTTP admits `None` (public routes) and `Bearer` (an
/// `Authorization` header is an HTTP concept); the internal protocols
/// (call/cron/queue) admit `Internal`. `Signature` is still reserved.
pub fn scheme_admissible(protocol: &ServiceProtocol, scheme: Scheme) -> bool {
    match protocol {
        ServiceProtocol::Http => {
            matches!(
                scheme,
                Scheme::None | Scheme::Bearer | Scheme::Signature | Scheme::Oidc
            )
        }
        // v0.103 (D-B): a WebSocket upgrade authenticates via `None` (anonymous)
        // or `Bearer` — but the token is read from the `Sec-WebSocket-Protocol`
        // subprotocol, since a browser `WebSocket` cannot set an `Authorization`
        // header. `Signature` is rejected at the WS boundary: HMAC-over-body has
        // no body on a handshake.
        ServiceProtocol::WebSocket { .. } => {
            matches!(scheme, Scheme::None | Scheme::Bearer)
        }
        // Events track, slice 0 (spine #936): delivery is an internal,
        // runtime-triggered invocation, like `Call`/`Cron`/`Queue` — no
        // external network request, so no external auth scheme applies.
        // I3 (#1971): a `call` also admits a declared foreign caller.
        ServiceProtocol::Call => matches!(scheme, Scheme::Internal | Scheme::Foreign),
        ServiceProtocol::Cron | ServiceProtocol::Queue { .. } | ServiceProtocol::Events { .. } => {
            matches!(scheme, Scheme::Internal)
        }
    }
}

/// v0.53: the closed claim-predicate vocabulary for a refinement actor's `where`
/// clause (`actor Admin = User where hasClaim("admin")`). Claims are untyped
/// JSON, so the predicate is a closed set — `hasClaim`/`claimEquals` composed
/// with `&&`/`||`/`!` — checked against the *verified* JWT claims at the
/// boundary. A general typed-claims expression surface is a later slice.
#[derive(Debug, Clone)]
pub enum ClaimPredicate {
    /// `hasClaim("name")` — the claim is present and truthy.
    HasClaim(String),
    /// `claimEquals("name", "value")` — the claim string-equals `value`.
    ClaimEquals(String, String),
    And(Box<ClaimPredicate>, Box<ClaimPredicate>),
    Or(Box<ClaimPredicate>, Box<ClaimPredicate>),
    Not(Box<ClaimPredicate>),
}

fn claim_str_lit(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::StrLit(s) => Some(s.clone()),
        _ => None,
    }
}

/// Recognise the closed claim-predicate vocabulary in a refinement `where`
/// expression. `Err(span)` points at the first sub-expression outside the set
/// (for `bynk.actor.refinement_predicate_unsupported`).
pub fn parse_claim_predicate(e: &Expr) -> Result<ClaimPredicate, Span> {
    match &e.kind {
        ExprKind::Paren(inner) => parse_claim_predicate(inner),
        ExprKind::BinOp(BinOp::And, l, r) => Ok(ClaimPredicate::And(
            Box::new(parse_claim_predicate(l)?),
            Box::new(parse_claim_predicate(r)?),
        )),
        ExprKind::BinOp(BinOp::Or, l, r) => Ok(ClaimPredicate::Or(
            Box::new(parse_claim_predicate(l)?),
            Box::new(parse_claim_predicate(r)?),
        )),
        ExprKind::UnaryOp(UnaryOp::Not, inner) => {
            Ok(ClaimPredicate::Not(Box::new(parse_claim_predicate(inner)?)))
        }
        ExprKind::Call {
            name,
            type_args,
            args,
        } if type_args.is_empty() => match (name.name.as_str(), args.as_slice()) {
            ("hasClaim", [a]) => claim_str_lit(a).map(ClaimPredicate::HasClaim).ok_or(a.span),
            ("claimEquals", [a, b]) => match (claim_str_lit(a), claim_str_lit(b)) {
                (Some(n), Some(v)) => Ok(ClaimPredicate::ClaimEquals(n, v)),
                (None, _) => Err(a.span),
                (_, None) => Err(b.span),
            },
            _ => Err(name.span),
        },
        _ => Err(e.span),
    }
}

/// Lower a claim predicate to a JavaScript boolean expression over `claims_var`
/// (the verified claims object, `Record<string, unknown>`). Used by the emitter
/// for the refinement seam's 403 check.
pub fn claim_predicate_to_js(pred: &ClaimPredicate, claims_var: &str) -> String {
    match pred {
        ClaimPredicate::HasClaim(name) => {
            format!(
                "globalThis.Boolean({claims_var}[\"{}\"])",
                js_str_escape(name)
            )
        }
        ClaimPredicate::ClaimEquals(name, value) => format!(
            "({claims_var}[\"{}\"] === \"{}\")",
            js_str_escape(name),
            js_str_escape(value)
        ),
        ClaimPredicate::And(l, r) => format!(
            "({} && {})",
            claim_predicate_to_js(l, claims_var),
            claim_predicate_to_js(r, claims_var)
        ),
        ClaimPredicate::Or(l, r) => format!(
            "({} || {})",
            claim_predicate_to_js(l, claims_var),
            claim_predicate_to_js(r, claims_var)
        ),
        ClaimPredicate::Not(inner) => {
            format!("(!{})", claim_predicate_to_js(inner, claims_var))
        }
    }
}

fn js_str_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// #2019: what a witness claim set requires of one claim. Each need is the
/// exact set of claim values that meets it, so two needs merge exactly when
/// their sets intersect. The excluded strings carried by `Truthy`, `Falsy`
/// and `NotEquals` come from negated `claimEquals`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ClaimNeed {
    /// String-equal to the value (`claimEquals`).
    Equals(String),
    /// Present and truthy (`hasClaim`), and none of these strings.
    Truthy(BTreeSet<String>),
    /// Absent or falsy (`!hasClaim`), and none of these strings.
    Falsy(BTreeSet<String>),
    /// Anything but these strings (`!claimEquals`), absent included.
    NotEquals(BTreeSet<String>),
}

type ClaimSet = std::collections::BTreeMap<String, ClaimNeed>;

/// The intersection of two needs on one claim, or `None` when it is empty.
fn merge_need(a: &ClaimNeed, b: &ClaimNeed) -> Option<ClaimNeed> {
    use ClaimNeed::*;
    let union = |x: &BTreeSet<String>, y: &BTreeSet<String>| x.union(y).cloned().collect();
    Some(match (a, b) {
        (Equals(x), Equals(y)) => (x == y).then(|| Equals(x.clone()))?,
        // A non-empty string is truthy; the empty string is falsy.
        (Equals(v), Truthy(not)) | (Truthy(not), Equals(v)) => {
            (!v.is_empty() && !not.contains(v)).then(|| Equals(v.clone()))?
        }
        (Equals(v), Falsy(not)) | (Falsy(not), Equals(v)) => {
            (v.is_empty() && !not.contains(v)).then(|| Equals(v.clone()))?
        }
        (Equals(v), NotEquals(not)) | (NotEquals(not), Equals(v)) => {
            (!not.contains(v)).then(|| Equals(v.clone()))?
        }
        (Truthy(_), Falsy(_)) | (Falsy(_), Truthy(_)) => return None,
        (Truthy(x), Truthy(y) | NotEquals(y)) | (NotEquals(y), Truthy(x)) => Truthy(union(x, y)),
        (Falsy(x), Falsy(y) | NotEquals(y)) | (NotEquals(y), Falsy(x)) => Falsy(union(x, y)),
        (NotEquals(x), NotEquals(y)) => NotEquals(union(x, y)),
    })
}

/// Merge two claim sets, or `None` when they contradict.
fn merge_claims(a: &ClaimSet, b: &ClaimSet) -> Option<ClaimSet> {
    let mut out = a.clone();
    for (name, need) in b {
        let merged = match out.get(name) {
            None => need.clone(),
            Some(have) => merge_need(have, need)?,
        };
        out.insert(name.clone(), merged);
    }
    Some(out)
}

/// Every claim set satisfying `pred` (when `want` is true) or falsifying it,
/// one per way the `||`/`&&` alternatives can be met. The closed predicate
/// vocabulary keeps this small.
fn claim_witnesses(pred: &ClaimPredicate, want: bool) -> Vec<ClaimSet> {
    let one = |name: &str, need: ClaimNeed| vec![ClaimSet::from([(name.to_string(), need)])];
    let both = |l: Vec<ClaimSet>, r: Vec<ClaimSet>| {
        l.iter()
            .flat_map(|a| r.iter().filter_map(move |b| merge_claims(a, b)))
            .collect()
    };
    let either = |mut l: Vec<ClaimSet>, r: Vec<ClaimSet>| {
        l.extend(r);
        l
    };
    match (pred, want) {
        (ClaimPredicate::HasClaim(n), true) => one(n, ClaimNeed::Truthy(BTreeSet::new())),
        (ClaimPredicate::ClaimEquals(n, v), true) => one(n, ClaimNeed::Equals(v.clone())),
        (ClaimPredicate::HasClaim(n), false) => one(n, ClaimNeed::Falsy(BTreeSet::new())),
        (ClaimPredicate::ClaimEquals(n, v), false) => {
            one(n, ClaimNeed::NotEquals(BTreeSet::from([v.clone()])))
        }
        (ClaimPredicate::And(l, r), true) | (ClaimPredicate::Or(l, r), false) => {
            both(claim_witnesses(l, want), claim_witnesses(r, want))
        }
        (ClaimPredicate::Or(l, r), true) | (ClaimPredicate::And(l, r), false) => {
            either(claim_witnesses(l, want), claim_witnesses(r, want))
        }
        (ClaimPredicate::Not(inner), _) => claim_witnesses(inner, !want),
    }
}

/// #2019: a JavaScript object literal of JWT claims that satisfies a
/// refinement actor's predicate, for the `system`-tier test driver to sign
/// next to `sub` when a case acts as that refinement (`by Admin("root")`):
/// `hasClaim("admin")` → `{ "admin": true }`, `claimEquals("role", "ops")` →
/// `{ "role": "ops" }`. The Worker's seam then checks the predicate against
/// the verified claims exactly as it would a real token's. A `Truthy` need
/// signs `true`: a boolean is never `===` a string, so it also lies outside
/// every excluded value. An absent claim meets `Falsy` and `NotEquals`, so
/// those sign nothing. The search is exact over the closed vocabulary, so
/// `None` means no claim set satisfies the predicate
/// (`hasClaim("a") && !hasClaim("a")`).
pub fn claim_witness_js(pred: &ClaimPredicate) -> Option<String> {
    let set = claim_witnesses(pred, true).into_iter().next()?;
    let entries = set
        .iter()
        .filter_map(|(name, need)| {
            let value = match need {
                ClaimNeed::Truthy(_) => "true".to_string(),
                ClaimNeed::Equals(v) => format!("\"{}\"", js_str_escape(v)),
                ClaimNeed::Falsy(_) | ClaimNeed::NotEquals(_) => return None,
            };
            Some(format!("\"{}\": {value}", js_str_escape(name)))
        })
        .collect::<Vec<_>>();
    Some(if entries.is_empty() {
        "{}".to_string()
    } else {
        format!("{{ {} }}", entries.join(", "))
    })
}

#[cfg(test)]
mod claim_witness_tests {
    use super::{ClaimPredicate as P, claim_witness_js};

    fn has(n: &str) -> Box<P> {
        Box::new(P::HasClaim(n.to_string()))
    }
    fn eq(n: &str, v: &str) -> Box<P> {
        Box::new(P::ClaimEquals(n.to_string(), v.to_string()))
    }

    #[test]
    fn a_single_claim_is_signed() {
        assert_eq!(
            claim_witness_js(&has("admin")).as_deref(),
            Some("{ \"admin\": true }")
        );
        assert_eq!(
            claim_witness_js(&eq("role", "ops")).as_deref(),
            Some("{ \"role\": \"ops\" }")
        );
    }

    #[test]
    fn a_conjunction_signs_both_and_a_disjunction_its_first_arm() {
        assert_eq!(
            claim_witness_js(&P::And(has("a"), eq("role", "ops"))).as_deref(),
            Some("{ \"a\": true, \"role\": \"ops\" }")
        );
        assert_eq!(
            claim_witness_js(&P::Or(has("a"), has("b"))).as_deref(),
            Some("{ \"a\": true }")
        );
    }

    #[test]
    fn a_negation_leaves_the_claim_out_and_steers_the_disjunction() {
        assert_eq!(
            claim_witness_js(&P::Not(has("banned"))).as_deref(),
            Some("{}")
        );
        // `(a || b) && !a` is met only by `b`.
        assert_eq!(
            claim_witness_js(&P::And(
                Box::new(P::Or(has("a"), has("b"))),
                Box::new(P::Not(has("a")))
            ))
            .as_deref(),
            Some("{ \"b\": true }")
        );
        // `!(a && b)` is met by leaving `a` out.
        assert_eq!(
            claim_witness_js(&P::Not(Box::new(P::And(has("a"), has("b"))))).as_deref(),
            Some("{}")
        );
    }

    #[test]
    fn a_negated_equality_excludes_one_value_only() {
        let not = |p: Box<P>| Box::new(P::Not(p));
        // `hasClaim("role") && !claimEquals("role", "banned")`: any truthy
        // non-"banned" value; `true` is never `===` a string.
        assert_eq!(
            claim_witness_js(&P::And(has("role"), not(eq("role", "banned")))).as_deref(),
            Some("{ \"role\": true }")
        );
        // `claimEquals("role", "ops") && !claimEquals("role", "admin")`.
        assert_eq!(
            claim_witness_js(&P::And(eq("role", "ops"), not(eq("role", "admin")))).as_deref(),
            Some("{ \"role\": \"ops\" }")
        );
        // `!claimEquals("role", "banned")` alone: leave the claim out.
        assert_eq!(
            claim_witness_js(&P::Not(eq("role", "banned"))).as_deref(),
            Some("{}")
        );
        // `!claimEquals("r", "x") && !hasClaim("r")`: absent meets both.
        assert_eq!(
            claim_witness_js(&P::And(not(eq("r", "x")), not(has("r")))).as_deref(),
            Some("{}")
        );
    }

    #[test]
    fn an_excluded_value_still_contradicts_its_own_equality() {
        let not = |p: Box<P>| Box::new(P::Not(p));
        // `hasClaim("r") && !claimEquals("r", "x") && claimEquals("r", "x")`.
        assert_eq!(
            claim_witness_js(&P::And(
                Box::new(P::And(has("r"), not(eq("r", "x")))),
                eq("r", "x")
            )),
            None
        );
        // An empty string is falsy, so it cannot meet `hasClaim`.
        assert_eq!(claim_witness_js(&P::And(has("r"), eq("r", ""))), None);
        assert_eq!(
            claim_witness_js(&P::And(not(has("r")), eq("r", ""))).as_deref(),
            Some("{ \"r\": \"\" }")
        );
    }

    #[test]
    fn a_contradiction_has_no_witness() {
        assert_eq!(
            claim_witness_js(&P::And(has("a"), Box::new(P::Not(has("a"))))),
            None
        );
        assert_eq!(claim_witness_js(&P::And(eq("r", "x"), eq("r", "y"))), None);
    }
}
