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
    /// #3386: a signed, expiring session cookie that Bynk both issues and
    /// verifies (HMAC-SHA-256 over a compact JWS). A browser credential, so
    /// HTTP only; single-actor only in the first slice (like `Oidc`).
    SignedCookie,
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
            "SignedCookie" => Scheme::SignedCookie,
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
                | Scheme::SignedCookie
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
            Scheme::SignedCookie => "SignedCookie",
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

/// The first-party record `Sender` yields (#3328): `{ address: String, dmarc:
/// DmarcVerdict }`, exported by `bynk.cloudflare`.
pub const SENDER_IDENTITY: &str = "SenderIdentity";

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
        // #3328 (DECISION E): the inbound-mail sender — Internal (the platform
        // delivers and authenticates the mail before the handler runs),
        // yielding the first-party `SenderIdentity` (envelope address plus the
        // DMARC verdict) when a handler binds it with `by s: Sender`.
        "Sender" => Contract {
            scheme: Scheme::Internal,
            identity: Identity::Declared(SENDER_IDENTITY.to_string()),
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
/// Q5). HTTP and WebSocket have no safe default — `by` is required there — and
/// neither has `call` (#3891, superseding ADR 0082's `Caller` default) nor `queue`
/// (#3899: who may send to a consumer is an access decision) nor email (#3888:
/// whether to act on mail that failed DMARC is a written decision).
pub fn default_actor(protocol: &ServiceProtocol) -> Option<&'static str> {
    match protocol {
        ServiceProtocol::Cron => Some("Scheduler"),
        // Events track, slice 0 (spine #936): delivery is runtime-triggered,
        // not an external caller — like `Queue`'s "Producer", the default
        // actor names the event's originating publisher.
        ServiceProtocol::Events { .. } => Some("Publisher"),
        // v0.103: like HTTP, a WebSocket upgrade has no safe default actor —
        // `by` is mandatory on `on open` (edge auth before accept, D-A).
        ServiceProtocol::Call
        | ServiceProtocol::Http
        | ServiceProtocol::Queue { .. }
        | ServiceProtocol::WebSocket { .. }
        | ServiceProtocol::Email { .. } => None,
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
    /// #3821: the `iss`/`aud` the actor's tokens must carry, and the variable that can replace them.
    pub claims: BearerClaims,
}

/// #3821: what a `Bearer` actor declares beyond its secret: the `issuer` and `audience` a token must
/// carry, and the `trustFrom` environment variable whose JSON `{ issuer?, audience? }` replaces them.
/// All `None` means the token's `iss` and `aud` are not checked, as before.
#[derive(Debug, Clone, Default)]
pub struct BearerClaims {
    pub issuer: Option<String>,
    pub audience: Option<String>,
    pub trust_from: Option<String>,
}

impl BearerClaims {
    /// The claims `actor` declares; a refinement is read through its base by the caller.
    pub fn of(actor: &ActorDecl) -> Self {
        let get = |key: &str| {
            actor
                .scheme_arg(key)
                .and_then(|a| a.value.as_str())
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        };
        Self {
            issuer: get("issuer"),
            audience: get("audience"),
            trust_from: get("trustFrom"),
        }
    }

    /// Whether the actor checks no claim beyond the signature, so its verification is unchanged.
    pub fn is_unchecked(&self) -> bool {
        self.issuer.is_none() && self.audience.is_none() && self.trust_from.is_none()
    }
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

/// #3052: whether a `by` clause names a single local `Oidc` actor (an `Oidc`
/// actor is never a sum member or a refinement base), so the `system` driver
/// can mint a token for it and its seam refuses a bad one.
pub fn by_clause_is_oidc(by: &ByClause, actors: &HashMap<String, ActorDecl>) -> bool {
    !by.is_sum()
        && actors
            .get(&by.primary().name)
            .and_then(|a| a.auth.as_ref())
            .and_then(|a| Scheme::from_name(a.name.as_str()))
            == Some(Scheme::Oidc)
}

/// Whether a route's seam refuses a forged or expired credential as a `401`: a `Bearer` route's
/// token, or (#3386) a `SignedCookie` route's cookie.
pub fn route_refuses_bad_token(by: &ByClause, actors: &HashMap<String, ActorDecl>) -> bool {
    route_has_bearer_seam(by, actors) || by_clause_is_signed_cookie(by, actors)
}

/// #3386: whether a `by` clause names a single local `SignedCookie` actor (never a sum member), so
/// the `system` driver can mint a session cookie for it and its seam refuses a bad one.
pub fn by_clause_is_signed_cookie(by: &ByClause, actors: &HashMap<String, ActorDecl>) -> bool {
    !by.is_sum()
        && actors
            .get(&by.primary().name)
            .and_then(|a| a.auth.as_ref())
            .and_then(|a| Scheme::from_name(a.name.as_str()))
            == Some(Scheme::SignedCookie)
}

/// #3938: whether a `by` clause names a single local `SignedCookie` actor with `sessions = true`
/// (never a sum member), whose seam can refuse a cookie for a revoked session.
pub fn by_clause_has_sessions(by: &ByClause, actors: &HashMap<String, ActorDecl>) -> bool {
    by_clause_is_signed_cookie(by, actors)
        && actors
            .get(&by.primary().name)
            .and_then(|a| signed_cookie_seam_of(a, None))
            .is_some_and(|s| s.sessions)
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
        claims: BearerClaims::of(base),
    })
}

/// #3904: what a hosting Durable Object needs of a lone `from websocket` credential seam: the binder
/// and the identity type, carried in a [`BearerSeam`] whatever the scheme (`Bearer`, `Oidc` or
/// `SignedCookie`). The Worker verified the credential, so the Durable Object reads none of the
/// seam's trust roots: `secret` is empty and there is no authorisation predicate for the other two.
pub fn ws_principal_seam_for(
    handler: &Handler,
    actors: &HashMap<String, ActorDecl>,
) -> Option<BearerSeam> {
    if let Some(seam) = bearer_seam_for(handler, actors) {
        return Some(seam);
    }
    let (binder, identity_type) = match (
        oidc_seam_for(handler, actors),
        signed_cookie_seam_for(handler, actors),
    ) {
        (Some(o), _) => (o.binder, o.identity_type),
        (None, Some(c)) => (c.binder, c.identity_type),
        (None, None) => return None,
    };
    Some(BearerSeam {
        binder,
        secret: String::new(),
        identity_type,
        authorization: None,
        claims: BearerClaims::default(),
    })
}

/// #3821: the `iss`/`aud` the lone `Bearer` actor of a `by` clause declares (a refinement reads its
/// base's), for the test tier: `by WrongIssuer` and `by WrongAudience` drive such a route when it
/// declares the claim. `None` for a sum, and for any other scheme.
pub fn bearer_claims_of_route(
    by: &ByClause,
    actors: &HashMap<String, ActorDecl>,
) -> Option<BearerClaims> {
    if by.is_sum() {
        return None;
    }
    let named = actors.get(&by.primary().name)?;
    let base = match &named.refinement {
        Some(r) => actors.get(&r.base.name)?,
        None => named,
    };
    (Scheme::from_name(base.auth.as_ref()?.name.as_str()) == Some(Scheme::Bearer))
        .then(|| BearerClaims::of(base))
}

/// The cookie name a `SignedCookie` actor issues and reads when `cookie` is not given.
pub const SIGNED_COOKIE_DEFAULT_NAME: &str = "__Host-session";

/// #3386: the data the emitter needs to lower a `SignedCookie` verification seam:
/// the `by` binder, the actor's name (the cookie's `aud`), the cookie's name, the
/// signing and previous-key env names, the `ttl` in seconds and the identity type
/// minted from the verified `sub`. Resolved only for a handler whose `by` clause
/// names a single local `SignedCookie` actor (it is never a sum member or a
/// refinement base); the checker guarantees the secret and `ttl` are present and
/// the identity is a string-constructible local type.
#[derive(Debug, Clone)]
pub struct SignedCookieSeam {
    /// The identity binder, or `None` for `by <Actor>` (verify, capture nothing).
    pub binder: Option<String>,
    /// The declaring actor's name: the cookie's `aud`.
    pub actor: String,
    /// The cookie's name.
    pub cookie: String,
    /// The env var holding the signing key.
    pub secret: String,
    /// The env var holding the key being rotated out, when declared.
    pub previous: Option<String>,
    /// How long an issued cookie is valid, in seconds.
    pub ttl_secs: i64,
    /// The context-owned, string-constructible identity type minted from `sub`.
    pub identity_type: String,
    /// The cookie's `SameSite`: `Lax` (the default), `Strict` or `Unrestricted`.
    pub same_site: String,
    /// #3938: `sessions = true`: the cookie carries a session id the seam checks against the
    /// generated `<Actor>Sessions` agent.
    pub sessions: bool,
}

/// The name of the compiler-generated session agent of `SignedCookie` actor `actor` (#3938).
pub fn sessions_agent_name(actor: &str) -> String {
    format!("{actor}Sessions")
}

/// Resolve a handler's `SignedCookie` seam, if its `by` clause names a single
/// local `SignedCookie` actor.
pub fn signed_cookie_seam_for(
    handler: &Handler,
    actors: &HashMap<String, ActorDecl>,
) -> Option<SignedCookieSeam> {
    let by = handler.by_clause.as_ref()?;
    if by.is_sum() {
        return None;
    }
    let actor = actors.get(&by.primary().name)?;
    signed_cookie_seam_of(actor, by.binder.as_ref().map(|b| b.name.clone()))
}

/// The seam of one `SignedCookie` actor declaration (shared by the handler
/// resolver and by an actor's `issue`/`expire` members).
pub fn signed_cookie_seam_of(
    actor: &ActorDecl,
    binder: Option<String>,
) -> Option<SignedCookieSeam> {
    if Scheme::from_name(actor.auth.as_ref()?.name.as_str()) != Some(Scheme::SignedCookie) {
        return None;
    }
    let secret = actor.scheme_arg("secret")?.value.as_str()?.to_string();
    let ttl_ms = actor.scheme_arg("ttl")?.value.as_duration_millis()?;
    let TypeRef::Named(id) = actor.identity.as_ref()? else {
        return None;
    };
    Some(SignedCookieSeam {
        binder,
        actor: actor.name.name.clone(),
        cookie: actor
            .scheme_arg("cookie")
            .and_then(|a| a.value.as_str())
            .unwrap_or(SIGNED_COOKIE_DEFAULT_NAME)
            .to_string(),
        secret,
        previous: actor
            .scheme_arg("previous")
            .and_then(|a| a.value.as_str())
            .map(str::to_string),
        ttl_secs: ttl_ms / 1000,
        identity_type: id.name.clone(),
        same_site: actor
            .scheme_arg("sameSite")
            .and_then(|a| a.value.as_ident())
            .unwrap_or("Lax")
            .to_string(),
        sessions: actor
            .scheme_arg("sessions")
            .and_then(|a| a.value.as_ident())
            == Some("true"),
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
    oidc_seam_of(actor, by.binder.as_ref().map(|b| b.name.clone()))
}

/// The seam of one `Oidc` actor declaration (a handler's single actor, or a member of a sum, #3936).
/// A refinement's `auth` is `None`, so it follows the Bearer refinement path; an `Oidc` base is not
/// narrowed.
pub fn oidc_seam_of(actor: &ActorDecl, binder: Option<String>) -> Option<OidcSeam> {
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
        binder,
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

/// #3937: the binder of an HTTP handler whose `by` clause names one actor that reads a verified
/// JWT, so the binder's `.claims` is the token's claims: an `Oidc` actor, or a `Bearer` actor or a
/// refinement of one. `None` for a sum (the binder there is the resolved arm), a handler of any
/// other protocol, and a `by` with no binder.
pub fn claims_binder_for(handler: &Handler, actors: &HashMap<String, ActorDecl>) -> Option<String> {
    // The verified claims reach a handler only on the HTTP path (a WebSocket upgrade forwards the
    // identity and expiry alone), so a `from websocket` binder has no `.claims` (#3904).
    if !matches!(handler.kind, HandlerKind::Http { .. }) {
        return None;
    }
    if let Some(seam) = oidc_seam_for(handler, actors) {
        return seam.binder;
    }
    if handler.by_clause.as_ref()?.is_sum() {
        return None;
    }
    bearer_seam_for(handler, actors)?.binder
}

impl crate::checker::HandlerBodyCheck<'_> {
    /// What the handler's `by` clause lets its body read of a verified token: the binder whose
    /// `.claims` is the token's (#2380, #3937), and the `Bearer` arms of an actor sum, whose pattern
    /// binds the claims beside the identity (#4344).
    pub fn with_claims_of(
        mut self,
        handler: &Handler,
        actors: &HashMap<String, ActorDecl>,
    ) -> Self {
        self.claims_binder = claims_binder_for(handler, actors);
        self.claims_sum_arms = claims_sum_arms_for(handler, actors);
        self
    }
}

/// #4344: the arms of an HTTP handler's actor sum (`by who: User | Visitor`) that read a verified token,
/// a `Bearer` or an `Oidc` arm, whose pattern may bind the token's claims beside the identity,
/// `User(u, c)`. Empty for every other handler.
pub fn claims_sum_arms_for(handler: &Handler, actors: &HashMap<String, ActorDecl>) -> Vec<String> {
    if !matches!(handler.kind, HandlerKind::Http { .. }) {
        return Vec::new();
    }
    sum_members_for(handler, actors)
        .into_iter()
        .flatten()
        .filter(|m| {
            matches!(
                m.seam,
                SumMemberSeam::Bearer { .. } | SumMemberSeam::Oidc(_)
            )
        })
        .map(|m| m.actor_name)
        .collect()
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
    // #3890: a refinement over `Caller` yields the same `CallerId`.
    let is_caller = (!actors.contains_key(name)
        && prelude_actor(name).map(|c| c.identity) == Some(Identity::CallerId)
        || actors
            .get(name)
            .is_some_and(|d| caller_refinement(d, actors).is_some()))
        && !handler.params.iter().any(|p| p.name.name == binder.name);
    is_caller.then(|| binder.name.clone())
}

/// #3328: the binder of a `from email` handler that captures the minted
/// `SenderIdentity` — `by s: Sender`, or `None` for a binder-less handler or a
/// handler that inherits the default (which captures nothing). `Sender` is a
/// prelude actor (a local actor of the same name is its own), and is admissible
/// only on `from email`, so the handler kind decides.
pub fn sender_binder_for(handler: &Handler, actors: &HashMap<String, ActorDecl>) -> Option<String> {
    if !matches!(handler.kind, HandlerKind::Message) {
        return None;
    }
    let by = handler.by_clause.as_ref()?;
    if by.is_sum() {
        return None;
    }
    let name = &by.primary().name;
    let refined = actors
        .get(name)
        .is_some_and(|decl| sender_refinement(decl, actors).is_some());
    // #3888: the prelude `Sender`, or a local refinement over it, mints the same `SenderIdentity`.
    let is_sender = (name == "Sender" && !actors.contains_key(name)) || refined;
    // A refinement checks the identity's verdict before the handler runs, so a binder-less
    // handler still receives it, under a name no source can write.
    let binder = match by.binder.as_ref() {
        Some(b) => b.name.clone(),
        None if refined => SENDER_REFINEMENT_BINDER.to_string(),
        None => return None,
    };
    (is_sender && !handler.params.iter().any(|p| p.name.name == binder)).then_some(binder)
}

/// The synthetic binder a binder-less `by <Sender refinement>` handler receives its identity under.
pub const SENDER_REFINEMENT_BINDER: &str = "__sender";

/// The DMARC verdicts the handler's `Sender` refinement admits, or `None` when its actor is not
/// one (a plain `by Sender` admits all mail).
pub fn sender_admits_for(
    handler: &Handler,
    actors: &HashMap<String, ActorDecl>,
) -> Option<Vec<&'static str>> {
    // The check reads the identity, so it exists only where the identity is passed.
    sender_binder_for(handler, actors)?;
    let by = handler.by_clause.as_ref()?;
    let decl = actors.get(&by.primary().name)?;
    sender_refinement(decl, actors).map(|p| p.admitted())
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
    /// #3899: whether this sender signs queue messages to a `from queue` consumer, not calls
    /// (an actor admitted on both is listed as a caller).
    pub queue: bool,
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
        queue: false,
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
    /// #3890: when contexts are admitted only by a refinement over `Caller`
    /// (`actor Admin = Caller where IsContext(…)`), its predicate. `None` with
    /// `contexts` set admits every context. A handler has at most one
    /// `Internal` member, so one predicate is enough.
    pub caller_predicate: Option<CallerPredicate>,
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
    /// #3890: a refinement over `Caller`: a Bynk context the predicate admits,
    /// with its name as the identity, tagged by the refinement's name.
    Refined(String, CallerPredicate),
}

impl CallSumMember {
    /// The member's tag, the actor's name.
    pub fn actor_name(&self) -> &str {
        match self {
            CallSumMember::Caller => "Caller",
            CallSumMember::Foreign(f) => &f.actor,
            CallSumMember::Refined(name, _) => name,
        }
    }
}

impl CallAdmission {
    /// #3890: the tag the resolved `Internal` member carries in a sum: the
    /// refinement's name, or `Caller`.
    pub fn internal_tag(&self) -> &str {
        self.sum
            .iter()
            .flatten()
            .find_map(|m| match m {
                CallSumMember::Refined(name, _) => Some(name.as_str()),
                _ => None,
            })
            .unwrap_or("Caller")
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
            caller_predicate: None,
            foreign: Vec::new(),
            sum: None,
        });
    };
    let mut admission = CallAdmission {
        contexts: false,
        caller_predicate: None,
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
            // #3890: a refinement over `Caller` admits the contexts its
            // predicate admits.
            Some(decl) if caller_refinement(decl, actors).is_some() => {
                let pred = caller_refinement(decl, actors).expect("checked above");
                admission.contexts = true;
                admission.caller_predicate = Some(pred.clone());
                members.push(CallSumMember::Refined(actor.name.clone(), pred));
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

/// #3899: who a `from queue` consumer that admits only signed messages admits. Each message must
/// come from one of the foreign senders, signed under that actor's secret, or from a Bynk context a
/// `Producer` refinement's predicate admits, signed under the pair's edge key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueAdmission {
    /// The foreign senders admitted, in `by` order.
    pub foreign: Vec<ForeignCaller>,
    /// The `Producer` refinements admitted, in `by` order: the actor's name and the contexts its
    /// predicate admits.
    pub producers: Vec<(String, CallerPredicate)>,
    /// For a sum (`by who: Legacy | Orders`), the members in `by` order: the handler's `match`
    /// reads which one signed the message.
    pub sum: Option<Vec<CallSumMember>>,
}

impl QueueAdmission {
    /// Whether a Bynk context may send to the consumer: some `Producer` refinement is admitted.
    pub fn admits_contexts(&self) -> bool {
        !self.producers.is_empty()
    }

    /// Whether the Bynk context `context` is one a refinement admits.
    pub fn admits_context(&self, context: &str) -> bool {
        self.producers.iter().any(|(_, p)| p.admits(context))
    }
}

/// #3899: what a `from queue` consumer's `on message` admits, or `None` when it admits unsigned
/// messages (`by Producer`, today's behaviour). A consumer whose `by` names only `Foreign` actors
/// and `Producer` refinements admits only signed messages; the checker refuses a `by` that mixes
/// them with an unsigned actor, so a signed consumer's every member is a signed sender here.
pub fn queue_admission_for(
    handler: &Handler,
    protocol: &ServiceProtocol,
    actors: &HashMap<String, ActorDecl>,
) -> Option<QueueAdmission> {
    if !matches!(protocol, ServiceProtocol::Queue { .. }) {
        return None;
    }
    queue_admission_of(handler, actors)
}

/// [`queue_admission_for`] without the protocol, for a caller that has only the handler (the seam
/// lowering): an `on message` whose `by` names `Foreign` actors or `Producer` refinements is a queue
/// consumer's, because the checker admits neither on any other handler but `call`.
pub fn queue_admission_of(
    handler: &Handler,
    actors: &HashMap<String, ActorDecl>,
) -> Option<QueueAdmission> {
    if !matches!(handler.kind, HandlerKind::Message) {
        return None;
    }
    let by = handler.by_clause.as_ref()?;
    let mut foreign = Vec::new();
    let mut producers = Vec::new();
    let mut members = Vec::new();
    for actor in &by.actors {
        // A prelude actor (`Producer`) or a local non-`Foreign` one admits unsigned messages.
        let decl = actors.get(&actor.name)?;
        if let Some(pred) = producer_refinement(decl, actors) {
            producers.push((actor.name.clone(), pred.clone()));
            members.push(CallSumMember::Refined(actor.name.clone(), pred));
            continue;
        }
        if decl.auth.as_ref().and_then(|a| Scheme::from_name(&a.name)) != Some(Scheme::Foreign) {
            return None;
        }
        // Classified by scheme, not by whether the declaration is whole, as `call_admission_for`
        // does: a malformed `Foreign` actor (the checker reports it) is still a signed sender, with
        // an empty secret that no environment supplies, so the consumer refuses every message.
        let f = ForeignCaller {
            queue: true,
            ..foreign_caller_of(decl).unwrap_or_else(|| ForeignCaller {
                actor: decl.name.name.clone(),
                secret: String::new(),
                worker: None,
                queue: true,
            })
        };
        members.push(CallSumMember::Foreign(f.clone()));
        foreign.push(f);
    }
    let sum = by.is_sum().then_some(members);
    (!foreign.is_empty() || !producers.is_empty()).then_some(QueueAdmission {
        foreign,
        producers,
        sum,
    })
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

/// #2864: whether a `by` clause's route has a `Bearer` seam the `system`
/// driver can forge or expire a token for — a lone Bearer actor (or a
/// refinement of one), or an actor sum with a Bearer member and no open
/// member ([`sum_has_open_member`]), whose seam never refuses a bad token.
pub fn route_has_bearer_seam(by: &ByClause, actors: &HashMap<String, ActorDecl>) -> bool {
    if !by.is_sum() {
        return by_clause_is_bearer(by, actors);
    }
    if sum_has_open_member(by, actors) {
        return false;
    }
    by.actors.iter().any(|a| {
        actors
            .get(&a.name)
            .and_then(|d| d.auth.as_ref())
            .and_then(|auth| Scheme::from_name(auth.name.as_str()))
            == Some(Scheme::Bearer)
    })
}

/// #2155: whether a `by` clause's route has a credential seam the `system`
/// driver can leave out (`by Nobody`): a Bearer actor (or a refinement of
/// one), an `Oidc` actor (#3052), a `Signature` actor, or an actor sum with such a member and no open
/// member ([`sum_has_open_member`]).
pub fn route_is_secured(by: &ByClause, actors: &HashMap<String, ActorDecl>) -> bool {
    if !by.is_sum() {
        return by_clause_is_bearer(by, actors)
            || by_clause_is_oidc(by, actors)
            || by_clause_is_signed_cookie(by, actors)
            || signature_seam_for_by(by, actors).is_some();
    }
    if sum_has_open_member(by, actors) {
        return false;
    }
    by.actors.iter().any(|a| {
        actors
            .get(&a.name)
            .and_then(|d| d.auth.as_ref())
            .and_then(|auth| Scheme::from_name(auth.name.as_str()))
            .is_some_and(|s| {
                // #3936: an `Oidc` or `SignedCookie` member refuses a missing credential too.
                matches!(
                    s,
                    Scheme::Bearer | Scheme::Signature | Scheme::Oidc | Scheme::SignedCookie
                )
            })
    })
}

/// #3922: whether a route's responses carry one caller's data, so a shared cache must not
/// store them: its `by` clause names a `Bearer`, `Oidc`, `SignedCookie` or `Signature`
/// actor (a refinement takes its base's scheme), or is a sum with such a member. A
/// `Visitor` route, and a sum of open actors, are not.
pub fn route_is_credentialed(by: &ByClause, actors: &HashMap<String, ActorDecl>) -> bool {
    by.actors.iter().any(|a| {
        let Some(decl) = actors.get(&a.name) else {
            return false;
        };
        let base = decl
            .refinement
            .as_ref()
            .and_then(|r| actors.get(&r.base.name))
            .unwrap_or(decl);
        base.auth
            .as_ref()
            .and_then(|auth| Scheme::from_name(auth.name.as_str()))
            .is_some_and(|s| {
                matches!(
                    s,
                    Scheme::Bearer | Scheme::Oidc | Scheme::SignedCookie | Scheme::Signature
                )
            })
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
        /// #3821: the `iss`/`aud` the member's tokens must carry.
        claims: BearerClaims,
    },
    Signature(SignatureSeam),
    /// #3936: an `Oidc` member, verified by the same JWKS seam as a single-actor route (no binder:
    /// the sum's `who` carries the identity).
    Oidc(OidcSeam),
    /// #3936: a `SignedCookie` member. Its CSRF check belongs to this arm: it runs when the arm is
    /// tried and a session cookie is present.
    SignedCookie(SignedCookieSeam),
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
            SumMemberSeam::Oidc(seam) => Some(&seam.identity_type),
            SumMemberSeam::SignedCookie(seam) => Some(&seam.identity_type),
            SumMemberSeam::None | SumMemberSeam::Signature(_) => None,
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
                        claims: BearerClaims::of(decl),
                    }
                }
                Scheme::Signature => SumMemberSeam::Signature(signature_seam_from_decl(decl)?),
                Scheme::Oidc => SumMemberSeam::Oidc(oidc_seam_of(decl, None)?),
                Scheme::SignedCookie => {
                    SumMemberSeam::SignedCookie(signed_cookie_seam_of(decl, None)?)
                }
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

/// #3994, #4266: the credential headers a route's seam consumes, each with the actor that consumes
/// it, in a stable order and each header once: `authorization` for a `Bearer` or `Oidc` actor, the
/// signature header for a `Signature` one. An actor sum hides what any member reads, because the
/// entry builds the request's header view before the sum's wrapper resolves which member matched.
pub fn consumed_credential_headers(
    handler: &Handler,
    actors: &HashMap<String, ActorDecl>,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut hide = |name: &str, actor: &str| {
        let name = name.to_ascii_lowercase();
        if !out.iter().any(|(h, _)| *h == name) {
            out.push((name, actor.to_string()));
        }
    };
    if let Some(members) = sum_members_for(handler, actors) {
        for m in &members {
            match &m.seam {
                SumMemberSeam::Bearer { .. } | SumMemberSeam::Oidc(_) => {
                    hide("authorization", &m.actor_name)
                }
                SumMemberSeam::Signature(s) => hide(&s.header, &m.actor_name),
                // #3936: the cookie arm's cookie is hidden by name, not by header.
                SumMemberSeam::SignedCookie(_) | SumMemberSeam::None => {}
            }
        }
        return out;
    }
    let primary = handler
        .by_clause
        .as_ref()
        .map(|by| by.primary().name.clone())
        .unwrap_or_default();
    if bearer_seam_for(handler, actors).is_some() || oidc_seam_for(handler, actors).is_some() {
        hide("authorization", &primary);
    }
    if let Some(s) = signature_seam_for(handler, actors) {
        hide(&s.header, &primary);
    }
    out
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
                Scheme::None
                    | Scheme::Bearer
                    | Scheme::Signature
                    | Scheme::Oidc
                    | Scheme::SignedCookie
            )
        }
        // v0.103 (D-B): a WebSocket upgrade authenticates via `None` (anonymous)
        // or `Bearer` — but the token is read from the `Sec-WebSocket-Protocol`
        // subprotocol, since a browser `WebSocket` cannot set an `Authorization`
        // header. `Signature` is rejected at the WS boundary: HMAC-over-body has
        // no body on a handshake.
        // #3904: a browser also authenticates an upgrade with a session cookie it sends by itself
        // (`SignedCookie`, behind an `Origin` check) or a provider's token through the
        // `bynk.oidc` subprotocol pair (`Oidc`), each a single actor; a sum takes only `None`
        // and `Bearer` members.
        ServiceProtocol::WebSocket { .. } => {
            matches!(
                scheme,
                Scheme::None | Scheme::Bearer | Scheme::Oidc | Scheme::SignedCookie
            )
        }
        // Events track, slice 0 (spine #936): delivery is an internal,
        // runtime-triggered invocation, like `Call`/`Cron`/`Queue` — no
        // external network request, so no external auth scheme applies.
        // I3 (#1971): a `call` also admits a declared foreign caller.
        ServiceProtocol::Call => matches!(scheme, Scheme::Internal | Scheme::Foreign),
        // #3899: a queue consumer may name a `Foreign` producer, whose messages it admits only when
        // signed under its secret; the checker refuses the forms not yet enforced
        // (`queue_actor_not_enforced`).
        ServiceProtocol::Queue { .. } => matches!(scheme, Scheme::Internal | Scheme::Foreign),
        ServiceProtocol::Cron | ServiceProtocol::Events { .. } => {
            matches!(scheme, Scheme::Internal)
        }
        // #3328: the platform delivers inbound mail, so the only admitted
        // scheme is the platform's own (`Internal`), as `Sender` is. A
        // DMARC-refusing scheme could follow (DECISION E).
        ServiceProtocol::Email { .. } => matches!(scheme, Scheme::Internal),
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
            ..
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

/// #3890: the closed predicate vocabulary for a refinement over `Caller`
/// (`actor Admin = Caller where IsContext(shop.admin)`). `IsContext` is a
/// fixed, compiler-known check (capitalised, as `Matches` and `NonEmpty` are),
/// decided at compile time against the project's call graph, and again at run
/// time on the caller the signature proved. The combinators are the claim
/// predicates', so `!IsContext(shop.legacy)` reads as a deny-list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallerPredicate {
    /// `IsContext(a.b, c.d)` — the calling context is one of the named ones.
    IsContext(Vec<String>),
    And(Box<CallerPredicate>, Box<CallerPredicate>),
    Or(Box<CallerPredicate>, Box<CallerPredicate>),
    Not(Box<CallerPredicate>),
}

impl CallerPredicate {
    /// Whether the named calling context passes the predicate.
    pub fn admits(&self, context: &str) -> bool {
        match self {
            CallerPredicate::IsContext(names) => names.iter().any(|n| n == context),
            CallerPredicate::And(l, r) => l.admits(context) && r.admits(context),
            CallerPredicate::Or(l, r) => l.admits(context) || r.admits(context),
            CallerPredicate::Not(inner) => !inner.admits(context),
        }
    }

    /// Every context name the predicate mentions, in source order.
    pub fn mentioned(&self) -> Vec<&str> {
        match self {
            CallerPredicate::IsContext(names) => names.iter().map(String::as_str).collect(),
            CallerPredicate::And(l, r) | CallerPredicate::Or(l, r) => {
                let mut out = l.mentioned();
                out.extend(r.mentioned());
                out
            }
            CallerPredicate::Not(inner) => inner.mentioned(),
        }
    }
}

/// #3888: the closed predicate vocabulary for a refinement over `Sender`
/// (`actor Authenticated = Sender where Dmarc(DmarcPass)`). `Dmarc` is a fixed,
/// compiler-known check (capitalised, as `IsContext` is). The DMARC verdict
/// has three values, so a predicate is decided by evaluating it against each
/// one: the emitted route carries the verdicts it admits, and the Worker
/// refuses other mail before the handler runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SenderPredicate {
    /// `Dmarc(DmarcPass, …)` — the verdict is one of those listed.
    Dmarc(Vec<String>),
    And(Box<SenderPredicate>, Box<SenderPredicate>),
    Or(Box<SenderPredicate>, Box<SenderPredicate>),
    Not(Box<SenderPredicate>),
}

/// The `DmarcVerdict` variants, as a handler's `s.identity.dmarc` reads them.
pub const DMARC_VERDICTS: [&str; 3] = ["DmarcPass", "DmarcFail", "DmarcNone"];

impl SenderPredicate {
    /// Whether mail with the named verdict passes the predicate.
    pub fn admits(&self, verdict: &str) -> bool {
        match self {
            SenderPredicate::Dmarc(names) => names.iter().any(|n| n == verdict),
            SenderPredicate::And(l, r) => l.admits(verdict) && r.admits(verdict),
            SenderPredicate::Or(l, r) => l.admits(verdict) || r.admits(verdict),
            SenderPredicate::Not(inner) => !inner.admits(verdict),
        }
    }

    /// The verdicts the predicate admits, in `DMARC_VERDICTS` order.
    pub fn admitted(&self) -> Vec<&'static str> {
        DMARC_VERDICTS
            .into_iter()
            .filter(|v| self.admits(v))
            .collect()
    }
}

/// Recognise the `Sender` predicate vocabulary in a refinement `where`
/// expression. `Err(span)` points at the first sub-expression outside it,
/// including a `Dmarc` argument that is not a `DmarcVerdict` variant.
pub fn parse_sender_predicate(e: &Expr) -> Result<SenderPredicate, Span> {
    match &e.kind {
        ExprKind::Paren(inner) => parse_sender_predicate(inner),
        ExprKind::BinOp(BinOp::And, l, r) => Ok(SenderPredicate::And(
            Box::new(parse_sender_predicate(l)?),
            Box::new(parse_sender_predicate(r)?),
        )),
        ExprKind::BinOp(BinOp::Or, l, r) => Ok(SenderPredicate::Or(
            Box::new(parse_sender_predicate(l)?),
            Box::new(parse_sender_predicate(r)?),
        )),
        ExprKind::UnaryOp(UnaryOp::Not, inner) => Ok(SenderPredicate::Not(Box::new(
            parse_sender_predicate(inner)?,
        ))),
        ExprKind::Call {
            name,
            type_args,
            args,
            ..
        } if type_args.is_empty() && name.name == "Dmarc" && !args.is_empty() => args
            .iter()
            .map(|a| match &a.kind {
                ExprKind::Ident(id) if DMARC_VERDICTS.contains(&id.name.as_str()) => {
                    Ok(id.name.clone())
                }
                _ => Err(a.span),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(SenderPredicate::Dmarc),
        ExprKind::Call { name, .. } => Err(name.span),
        _ => Err(e.span),
    }
}

/// #3888: the predicate of a refinement over the prelude `Sender` actor, or
/// `None` for any other actor (a malformed predicate reads as `None` too: the
/// checker reports it).
pub fn sender_refinement(
    decl: &ActorDecl,
    actors: &HashMap<String, ActorDecl>,
) -> Option<SenderPredicate> {
    let r = decl.refinement.as_ref()?;
    if r.base.name != "Sender" || actors.contains_key("Sender") {
        return None;
    }
    parse_sender_predicate(&r.predicate).ok()
}

/// A qualified name written as an expression: `shop.admin` is a field access
/// on an identifier.
pub fn dotted_name(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Ident(id) => Some(id.name.clone()),
        ExprKind::FieldAccess { receiver, field } => {
            Some(format!("{}.{}", dotted_name(receiver)?, field.name))
        }
        _ => None,
    }
}

/// Recognise the `Caller` predicate vocabulary in a refinement `where`
/// expression. `Err(span)` points at the first sub-expression outside it (for
/// `bynk.actor.refinement_predicate_unsupported`).
pub fn parse_caller_predicate(e: &Expr) -> Result<CallerPredicate, Span> {
    match &e.kind {
        ExprKind::Paren(inner) => parse_caller_predicate(inner),
        ExprKind::BinOp(BinOp::And, l, r) => Ok(CallerPredicate::And(
            Box::new(parse_caller_predicate(l)?),
            Box::new(parse_caller_predicate(r)?),
        )),
        ExprKind::BinOp(BinOp::Or, l, r) => Ok(CallerPredicate::Or(
            Box::new(parse_caller_predicate(l)?),
            Box::new(parse_caller_predicate(r)?),
        )),
        ExprKind::UnaryOp(UnaryOp::Not, inner) => Ok(CallerPredicate::Not(Box::new(
            parse_caller_predicate(inner)?,
        ))),
        ExprKind::Call {
            name,
            type_args,
            args,
            ..
        } if type_args.is_empty() && name.name == "IsContext" && !args.is_empty() => args
            .iter()
            .map(|a| dotted_name(a).ok_or(a.span))
            .collect::<Result<Vec<_>, _>>()
            .map(CallerPredicate::IsContext),
        ExprKind::Call { name, .. } => Err(name.span),
        _ => Err(e.span),
    }
}

/// #3890: the predicate of a refinement over the prelude `Caller` actor, or
/// `None` for any other actor (a malformed predicate reads as `None` too: the
/// checker reports it).
pub fn caller_refinement(
    decl: &ActorDecl,
    actors: &HashMap<String, ActorDecl>,
) -> Option<CallerPredicate> {
    let r = decl.refinement.as_ref()?;
    if r.base.name != "Caller" || actors.contains_key("Caller") {
        return None;
    }
    parse_caller_predicate(&r.predicate).ok()
}

/// #3899: the predicate of a refinement over the prelude `Producer` actor, or `None` for any
/// other actor (a malformed predicate reads as `None` too: the checker reports it). It uses the
/// same `IsContext(…)` vocabulary as a `Caller` refinement, over the context that signed the message.
pub fn producer_refinement(
    decl: &ActorDecl,
    actors: &HashMap<String, ActorDecl>,
) -> Option<CallerPredicate> {
    let r = decl.refinement.as_ref()?;
    if r.base.name != "Producer" || actors.contains_key("Producer") {
        return None;
    }
    parse_caller_predicate(&r.predicate).ok()
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
    let entries = claim_witness(pred)?
        .into_iter()
        .map(|(name, value)| {
            let value = match value {
                WitnessValue::True => "true".to_string(),
                WitnessValue::Str(v) => format!("\"{}\"", js_str_escape(&v)),
            };
            format!("\"{}\": {value}", js_str_escape(&name))
        })
        .collect::<Vec<_>>();
    Some(if entries.is_empty() {
        "{}".to_string()
    } else {
        format!("{{ {} }}", entries.join(", "))
    })
}

/// The value a witness claim is signed with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WitnessValue {
    /// The JSON boolean `true` (`hasClaim`).
    True,
    /// A JSON string (`claimEquals`).
    Str(String),
}

/// The claims, by name, that satisfy `pred` — the data behind
/// [`claim_witness_js`], shared with `bynk token` so a hand-minted token
/// carries exactly the claims the `system` harness signs. `None` when no
/// claim set satisfies the predicate.
pub fn claim_witness(pred: &ClaimPredicate) -> Option<Vec<(String, WitnessValue)>> {
    let set = claim_witnesses(pred, true).into_iter().next()?;
    Some(
        set.into_iter()
            .filter_map(|(name, need)| {
                let value = match need {
                    ClaimNeed::Truthy(_) => WitnessValue::True,
                    ClaimNeed::Equals(v) => WitnessValue::Str(v),
                    ClaimNeed::Falsy(_) | ClaimNeed::NotEquals(_) => return None,
                };
                Some((name, value))
            })
            .collect(),
    )
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

// ---- call-site principals (1A.33) --------------------------------------------

/// A reserved bad-credential principal (`by Forged`, …): it names no actor and takes no
/// identity, and the `system` driver presents a credential the seam must refuse.
///
/// #2135, #2864: `Forged` (a wrong secret), `Stale` (a timestamp past the tolerance) and
/// `Expired` (a Bearer or `Oidc` token whose `exp` has passed). #3052: `WrongIssuer` and
/// `WrongAudience` (an `Oidc` token for another issuer or audience). #3225:
/// `WrongAlgorithm` and `NoSubject` (an `Oidc` token under a refused algorithm, or with no
/// usable `sub`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BadCredential {
    Forged,
    Stale,
    Expired,
    WrongIssuer,
    WrongAudience,
    WrongAlgorithm,
    NoSubject,
    /// #3938: a cookie correctly signed for a session that was then revoked (`revokeAll`), so
    /// only a `SignedCookie` actor with `sessions = true` can refuse it.
    Revoked,
}

impl BadCredential {
    /// Every bad-credential principal.
    pub const ALL: [BadCredential; 8] = [
        BadCredential::Forged,
        BadCredential::Stale,
        BadCredential::Expired,
        BadCredential::WrongIssuer,
        BadCredential::WrongAudience,
        BadCredential::WrongAlgorithm,
        BadCredential::NoSubject,
        BadCredential::Revoked,
    ];

    /// The name written after `by`.
    pub const fn name(self) -> &'static str {
        match self {
            BadCredential::Forged => "Forged",
            BadCredential::Stale => "Stale",
            BadCredential::Expired => "Expired",
            BadCredential::WrongIssuer => "WrongIssuer",
            BadCredential::WrongAudience => "WrongAudience",
            BadCredential::WrongAlgorithm => "WrongAlgorithm",
            BadCredential::NoSubject => "NoSubject",
            BadCredential::Revoked => "Revoked",
        }
    }

    /// The `system` test driver that presents this credential.
    pub const fn driver(self) -> &'static str {
        match self {
            BadCredential::Forged => "forged",
            BadCredential::Stale => "stale",
            BadCredential::Expired => "expired",
            BadCredential::WrongIssuer => "wrongissuer",
            BadCredential::WrongAudience => "wrongaudience",
            BadCredential::WrongAlgorithm => "wrongalgorithm",
            BadCredential::NoSubject => "nosubject",
            BadCredential::Revoked => "revoked",
        }
    }

    /// Whether only an `Oidc` seam can refuse this credential.
    pub const fn oidc_only(self) -> bool {
        matches!(
            self,
            BadCredential::WrongIssuer
                | BadCredential::WrongAudience
                | BadCredential::WrongAlgorithm
                | BadCredential::NoSubject
        )
    }
}

/// #4385: the member a bad-credential principal names on an actor sum, `Session` in
/// `by Forged(Session)`: its argument when that is a bare identifier. A value (`by User("ada")`) is
/// an identity, never an arm.
pub fn bad_principal_arm(p: &bynk_syntax::ast::CallSiteActor) -> Option<&str> {
    match p.identity.as_deref().map(|e| &e.kind) {
        Some(ExprKind::Ident(id)) if p.claims.is_none() => Some(id.name.as_str()),
        _ => None,
    }
}

impl BadCredential {
    /// #4385: whether a member declared as `decl` has a seam that refuses this credential, for the
    /// principals a sum's arm can be named for. `Forged` reaches any of `Bearer`, `Signature`,
    /// `Oidc` and `SignedCookie`; `Expired` any but `Signature`; `Stale` a `Signature` declaring a
    /// `timestamp` and a `tolerance`; `Revoked` a `SignedCookie` with `sessions = true`. The
    /// `Oidc`-only principals are not offered on a sum.
    pub fn refused_by(self, decl: &ActorDecl) -> bool {
        let Some(scheme) = decl
            .auth
            .as_ref()
            .and_then(|a| Scheme::from_name(a.name.as_str()))
        else {
            return false;
        };
        match (self, scheme) {
            (
                BadCredential::Forged,
                Scheme::Bearer | Scheme::Signature | Scheme::Oidc | Scheme::SignedCookie,
            ) => true,
            (BadCredential::Expired, Scheme::Bearer | Scheme::Oidc | Scheme::SignedCookie) => true,
            (BadCredential::Stale, Scheme::Signature) => signature_seam_from_decl(decl)
                .is_some_and(|s| s.timestamp_header.is_some() && s.tolerance_secs.is_some()),
            (BadCredential::Revoked, Scheme::SignedCookie) => {
                signed_cookie_seam_of(decl, None).is_some_and(|s| s.sessions)
            }
            _ => false,
        }
    }
}

/// #4385: the members of the sum `by` whose seam refuses `bad`, in declared order. Empty when the
/// sum admits an open member (`Visitor`), whose catch-all means no credential is ever refused.
pub fn sum_refusing_arms(
    by: &ByClause,
    bad: BadCredential,
    actors: &HashMap<String, ActorDecl>,
) -> Vec<String> {
    if sum_has_open_member(by, actors) {
        return Vec::new();
    }
    by.actors
        .iter()
        .filter(|a| actors.get(&a.name).is_some_and(|d| bad.refused_by(d)))
        .map(|a| a.name.clone())
        .collect()
}

/// What a call-site principal (`by <Name>[(<identity>)]`) is: an actor, the reserved
/// no-credential principal `Nobody` (#706), or a reserved bad credential. The one
/// classification of those reserved names; the checker records it on the clause
/// ([`crate::checker::Callee::Principal`]) and every reader, checker and emitter, reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallSitePrincipal {
    /// An actor, by name (a declared actor of the target, or a prelude actor).
    Actor(String),
    /// `by Nobody`: no credential at all, so the auth seam answers `401`.
    Nobody,
    BadCredential(BadCredential),
    /// #4385: a bare bad credential on an actor sum whose one refusing member is not a `Bearer` or
    /// `Signature` seam, so the checker names that member: what the emitter drives as
    /// `by <Bad>(<member>)`. Only ever recorded on a clause, never classified from a name.
    BadCredentialArm(BadCredential, String),
}

impl CallSitePrincipal {
    /// Classify the name written after `by`.
    pub fn of(name: &str) -> CallSitePrincipal {
        if name == "Nobody" {
            return CallSitePrincipal::Nobody;
        }
        BadCredential::ALL
            .into_iter()
            .find(|b| b.name() == name)
            .map_or_else(
                || CallSitePrincipal::Actor(name.to_string()),
                CallSitePrincipal::BadCredential,
            )
    }

    /// Whether the principal drives the auth seam with no usable credential
    /// (`Nobody` or a bad credential), which makes an address's outcome an `HttpOutcome`.
    pub fn is_reserved(&self) -> bool {
        !matches!(self, CallSitePrincipal::Actor(_))
    }
}

#[cfg(test)]
mod call_site_principal_tests {
    use super::*;

    #[test]
    fn nobody_is_its_own_principal() {
        assert_eq!(CallSitePrincipal::of("Nobody"), CallSitePrincipal::Nobody);
        assert!(CallSitePrincipal::Nobody.is_reserved());
    }

    #[test]
    fn each_bad_credential_classifies_and_names_its_driver() {
        let drivers = [
            ("Forged", "forged"),
            ("Stale", "stale"),
            ("Expired", "expired"),
            ("WrongIssuer", "wrongissuer"),
            ("WrongAudience", "wrongaudience"),
            ("WrongAlgorithm", "wrongalgorithm"),
            ("NoSubject", "nosubject"),
            ("Revoked", "revoked"),
        ];
        assert_eq!(drivers.len(), BadCredential::ALL.len());
        for (name, driver) in drivers {
            let CallSitePrincipal::BadCredential(b) = CallSitePrincipal::of(name) else {
                panic!("{name} is a bad credential")
            };
            assert_eq!((b.name(), b.driver()), (name, driver));
            assert!(CallSitePrincipal::BadCredential(b).is_reserved());
        }
    }

    #[test]
    fn only_the_oidc_credentials_are_oidc_only() {
        let oidc: Vec<_> = BadCredential::ALL
            .into_iter()
            .filter(|b| b.oidc_only())
            .map(BadCredential::name)
            .collect();
        assert_eq!(
            oidc,
            [
                "WrongIssuer",
                "WrongAudience",
                "WrongAlgorithm",
                "NoSubject"
            ]
        );
    }

    /// An ordinary actor (declared or prelude, including one whose name merely contains a
    /// reserved word) is an actor, not a reserved principal.
    #[test]
    fn an_ordinary_actor_is_not_reserved() {
        for name in [
            "Visitor",
            "Caller",
            "User",
            "NobodyElse",
            "forged",
            "Stale2",
            "",
        ] {
            let p = CallSitePrincipal::of(name);
            assert_eq!(p, CallSitePrincipal::Actor(name.to_string()), "{name}");
            assert!(!p.is_reserved());
        }
    }
}
