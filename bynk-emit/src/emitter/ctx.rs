//! The module-invariant lowering context and the per-body state structs
//! that sit between it and `LowerCtx`.

use super::*;

/// Lowering state that is genuinely **module-invariant**: the same value at
/// every body-lowering site within one emitted module, and never written by the
/// recursive lowering itself. Grouped out of [`LowerCtx`] so a new lowering kind
/// inherits the whole set wholesale instead of re-deriving each default.
///
/// Nothing the lowering mutates *per body* may move in here — see the
/// scratch-state fields at the bottom of [`LowerCtx`]. A `ModuleCtx` is still
/// built fresh alongside each `LowerCtx` today, but filing a per-body counter
/// under a name that says "module" is exactly how such state starts leaking
/// between handler bodies.
pub(crate) struct ModuleCtx<'a> {
    /// Typed-commons handle (used to look up receiver types for method-call
    /// UFCS lowering).
    pub(super) commons: &'a TypedCommons,
    /// Cross-context info for v0.6 cross-context call lowering.
    pub(super) cross_context: &'a bynk_check::resolver::CrossContextInfo,
    /// The emitted module's conditional-runtime-helper accumulator.
    ///
    /// The `Bytes` lowerings (kernel, `==`, base64 codec) call
    /// [`RuntimeUse::note_bytes`] through this, so the module's import line is
    /// decided from what lowering actually emitted rather than by scanning the
    /// generated text for the helper's own name.
    ///
    /// Required rather than optional: a missing accumulator would make
    /// `note_bytes` a silent no-op, which is exactly the failure this replaces —
    /// a module that references `__bynkBytesEqual` without importing it. A
    /// lowering whose imports are decided elsewhere (the test scaffolds) owns a
    /// throwaway one, which reads as the deliberate choice it is.
    pub(super) runtime_use: &'a RuntimeUse,
    /// v0.8 build target. In workers mode cross-context calls lower to
    /// `callService(...)` instead of `deps.surface.<key>.<method>(...)`.
    pub(super) target: BuildTarget,
    /// #527: agent → method → the method's `given` caps (mirrors
    /// [`crate::emit_model::EmitProjectCtx::agent_method_givens`]). Consulted by
    /// the agent-call lowering to record capability requirements.
    pub(super) agent_method_givens: HashMap<String, HashMap<String, Vec<bynk_model::CapRefIr>>>,
    /// Events slice 3b (#978): each locally-declared event's resolved
    /// `@schema(N)` version (mirrors
    /// [`crate::emit_model::EmitProjectCtx::event_schema_versions`]). Default-
    /// empty like `agent_method_givens`, not a required constructor
    /// parameter like `runtime_use` — a miss here degrades to `schemaVersion:
    /// 1`, exactly every event's behaviour before this map existed, not a
    /// hard failure the way a missing `runtime_use` would be.
    pub(super) event_schema_versions: HashMap<String, i64>,
    /// #2058: the namespace each consumed context's type is named under in
    /// this module (mirrors [`crate::emitter::emit::consumed_type_ns`]).
    /// Default-empty: a miss names the type bare, as before.
    pub(super) consumed_type_ns: HashMap<String, String>,
    /// #2887: a consumed type this module renames, alias → the name its
    /// context declares and exports.
    pub(super) consumed_type_declared: HashMap<String, String>,
    /// #934: true when the unit being emitted is the reserved first-party
    /// `bynk` adapter itself. `bynk` is a reserved namespace, so a capability
    /// literally named `Idempotency` declared *in this unit* is unambiguously
    /// the real one — used alongside `CrossContextInfo::flattened_caps` (the
    /// consumed-from-elsewhere case) to confirm a flattened `Idempotency`
    /// call is genuinely first-party before scoping its key, not a same-named
    /// capability some other adapter or context happens to declare.
    pub(super) in_bynk_unit: bool,
    /// #1961: [`crate::emit_model::EmitProjectCtx::agent_called_contexts`].
    pub(super) agent_called_contexts: Vec<String>,
    /// #2045: [`crate::emit_model::EmitProjectCtx::consumed_context_types`]. A
    /// variant of one of these sums is built inline, not through the callee's
    /// companion object, so the callee's module is never a value import.
    pub(super) consumed_context_types: HashSet<String>,
    /// #3580: [`crate::emit_model::EmitProjectCtx::trace`]. `None` (the default)
    /// emits no trace recording.
    pub(super) trace: Option<crate::emit_model::TraceLoc>,
}

impl<'a> ModuleCtx<'a> {
    /// What the unit being emitted tells every body it lowers: whether it is the
    /// reserved `bynk` adapter, and (#3580) where a traced build names call sites.
    pub(super) fn note_unit(&mut self, ctx: &EmitProjectCtx) {
        self.in_bynk_unit = ctx.commons_name == "bynk";
        self.trace = ctx.trace.clone();
    }

    pub(super) fn new(
        commons: &'a TypedCommons,
        cross_context: &'a bynk_check::resolver::CrossContextInfo,
        runtime_use: &'a RuntimeUse,
    ) -> Self {
        Self {
            commons,
            cross_context,
            runtime_use,
            target: BuildTarget::Bundle,
            agent_method_givens: HashMap::new(),
            event_schema_versions: HashMap::new(),
            consumed_type_ns: HashMap::new(),
            consumed_type_declared: HashMap::new(),
            in_bynk_unit: false,
            agent_called_contexts: Vec::new(),
            consumed_context_types: HashSet::new(),
            trace: None,
        }
    }
}

/// The lowering state shared by the four **capability-bearing** body kinds — a
/// service handler, a composed provider op, an agent handler, and a websocket
/// lifecycle DO method. Every other kind carries no `HandlerShared` at all, and
/// the [`LowerCtx`] accessors below hand those kinds the same defaults the flat
/// struct used to give them (an empty capability set, no scope, `deps`).
#[derive(Default)]
pub(crate) struct HandlerShared {
    /// Names of capabilities in scope as `given C1, C2, ...`. Used to lower
    /// `Capability.op(args)` calls to `deps.Capability.op(args)`.
    pub(super) capabilities: HashSet<String>,
    /// #934: the calling handler's own qualified name (`<unit>.<service or
    /// agent>.<handler>`, e.g. `shop.reserve.ordering.call`). Read only by the
    /// `Idempotency.dedup`/`remember` lowering, which prefixes the
    /// developer-supplied key with it so two unrelated handlers using the same
    /// literal key never collide (ADR 0283; the retired idempotency track's
    /// §3.4 in `design/archive/retired-tracks.md`). `None` anywhere a
    /// capability call cannot occur (a plain method, a free fn, an
    /// invariant/transition predicate, a static field initialiser) — those
    /// kinds carry no `HandlerShared`, and the accessor reports `None`.
    pub(super) handler_scope: Option<String>,
    /// Events track, slice 2 (spine #936): the qualified name of the unit
    /// this body is emitted into (`ctx.commons_name`), read by the
    /// `Events.emit[E](event)` lowering to mint the envelope's
    /// `publisherId`. Context-scoped rather than agent-scoped: `Events.emit`
    /// is legal from a plain, keyless service handler with no agent
    /// instance to report, so this is the only identity available
    /// uniformly at every legal emission site (an amendment to
    /// design/bynk-design-notes.md §7's "the publisher is the emitting
    /// agent" framing — see the events-envelope ADR). Always populated
    /// alongside `handler_scope` at every construction site; never `None`
    /// in practice for a body that could contain an `Events.emit` call.
    pub(super) owning_context: String,
    /// v0.12: whether the receiver expression a capability call resolves
    /// against is `this.deps` (a composed provider body) rather than `deps` (a
    /// handler body). The fact, not the string: `LowerCtx::cap_deps_expr` and
    /// `cap_deps_node` both read it (1A.6).
    pub(super) deps_on_this: bool,
    /// The consumed contexts the current handler calls across the boundary.
    /// Non-empty drives whether `deps` gets a `surface` field type; in
    /// workers mode it is also exactly the Service Bindings the handler's
    /// `deps.env` type names (#1961).
    pub(super) cross_contexts_called: std::collections::BTreeSet<String>,
    /// v0.9.2: set when the body instantiates a local agent. In workers mode
    /// this drives `env` (carrying the DO namespaces) into the handler's deps
    /// type so the agent factory can reach its Durable Object binding.
    pub(super) agents_instantiated: bool,
    /// #527: capabilities required by agent methods this body calls, keyed by
    /// deps key. After body lowering these widen the handler's deps *type* to
    /// match the runtime value compose builds (which always carried them).
    pub(super) agent_given_caps_used: std::collections::BTreeMap<String, bynk_model::CapRefIr>,
}

/// The lowering state shared by the three **generated test-scaffold** body
/// kinds — a `stub`/`where`/`requires` predicate value, a unit test case, and an
/// integration test case.
#[derive(Default)]
pub(crate) struct TestShared {
    /// True when lowering **any** generated test-scaffold body — distinct from
    /// `assert_loc`, which is only ever `Some` for the two real `case` bodies
    /// and carries an unrelated payload (a diagnostic location). Kept as its own
    /// field (Locale capability track, slice 1, #844 review) rather than
    /// overloading `assert_loc.is_some()`, since that conflated "has a location"
    /// with "is test scaffolding" for a caller that has no location to give.
    pub(super) test_scaffold: bool,
    /// v0.59: the source text and project-relative path of the file the body
    /// came from, so an `assert` can emit a real `path:line:col` location (for
    /// `--format json` click-through) rather than a bare byte offset. Stays
    /// `None` for a predicate scaffold, which emits no `assert`.
    pub(super) assert_loc: Option<AssertLoc>,
}

/// The `store`-agent sub-state of an [`BodyMode::AgentHandler`] body: present
/// only when the hosting agent is a `store` agent, absent for a plain
/// state-record agent (whose handler reads `currentState`/`self.state` instead).
pub(crate) struct AgentStoreState {
    /// v0.81 (storage track): the name of the mutable working-state variable
    /// (`__state`) and the set of `Cell` field names. A bare `Cell` read lowers
    /// to `<var>.<cell>`, and a `cell := v` write lowers to `<var>.<cell> = <v>`
    /// — read-your-writes via the in-memory record, flushed once at handler end
    /// (ADR 0109).
    pub(super) state: (String, HashSet<String>),
    /// v0.82 (ADR 0110): the agent's `store` `Map` field names. A method call
    /// whose receiver is one lowers to an entry operation over `__state.<map>`
    /// (a JSON-serialisable `Record<string, V>`), staged in the working record
    /// and flushed at commit like any other state field.
    pub(super) maps: HashSet<String>,
    /// v0.83: the agent's `store` `Set` field names. A method call whose
    /// receiver is one lowers to an entry operation over `__state.<set>` (a
    /// `Record<string, boolean>`), staged in the working record.
    pub(super) sets: HashSet<String>,
    /// v0.87 (ADR 0113): the agent's `store` `Cache` fields (name → ttl millis).
    /// A method call whose receiver is one lowers to an entry op over
    /// `__state.<cache>` (a `Record<string, { v, exp }>`), applying TTL expiry
    /// against the injected `Clock`.
    pub(super) caches: HashMap<String, i64>,
    /// v0.95 (ADR 0121): the agent's `store` `Log` fields (name → optional
    /// `@retain` millis). `<log>.append` pushes `{ t: now(), v }` to
    /// `__state.<log>` (an array) and prunes past the retain horizon; the
    /// time-window roots / builders lower to a query pipeline over the array.
    pub(super) logs: HashMap<String, Option<i64>>,
    /// v0.93 (ADR 0118): the agent's `@indexed` secondary indexes (map name →
    /// the value-record fields indexed on). A mutating op on the map maintains a
    /// sibling posting-list `Record<string, string[]>` per field (`<map>__idx_<f>`);
    /// an equality `filter` on an indexed field routes to a posting lookup.
    pub(super) indexes: HashMap<String, Vec<String>>,
    /// v0.104/v0.105 (real-time track slice 3b): the agent's held `store Map[K,
    /// Connection]` fields (name → the connection's **frame type** `F`, e.g.
    /// `ServerFrame`). On Workers these persist `K → connId` in the durable state
    /// record; a method call whose receiver is one lowers to an entry op over
    /// `__state.<map>` (the connId record) with `connIdOf`/`resolveConnection<F>` —
    /// not the plain `Record<string, V>` ops (held maps are excluded from
    /// [`AgentStoreState::maps`]).
    pub(super) held_maps: HashMap<String, String>,
    /// Review of #1460: a plain `store Map[K, V]` field's own value type `V`
    /// (name → its rendered TS type), for `lower_query_method`'s own
    /// collection-kernel sites (`distinctBy`/`joinOn`/`leftJoin`/`groupBy`)
    /// when the receiver is a plain map's own value scan lifted into the
    /// general query vocabulary — mirrors `held_maps`'s own construction
    /// exactly, just for the non-held case.
    pub(super) map_values: HashMap<String, String>,
    /// Review of #1460: a `store Log[T]` field's own element type `T` (name →
    /// its rendered TS type), for the identical reason `map_values` exists —
    /// a Log's value scan is also lifted into the general query vocabulary.
    pub(super) log_values: HashMap<String, String>,
}

/// What [`LowerCtx`] is lowering *right now*. One variant per real body-emission
/// site; each carries exactly the state that site populates and nothing else, so
/// "not applicable to this kind" is expressed in the type rather than left
/// indistinguishable from "deliberately defaulted".
pub(crate) enum BodyMode {
    /// A type's method body (`emit_method`).
    Method,
    /// A free function body (`emit_free_fn`).
    FreeFn,
    /// #2868: a context function that declares `given`: a free function whose
    /// hidden `deps` parameter carries exactly the capabilities it names, so a
    /// capability call lowers as in a handler (`deps.Logger.error(…)`).
    GivenFn { handler: HandlerShared },
    /// An agent field's static initialiser expression (`emit_agent`).
    StaticInit,
    /// v0.80: an agent invariant predicate. Carries the name of the
    /// proposed-state variable (the `commitState` parameter) and the set of
    /// state field names — a bare ident matching a state field lowers to
    /// `<var>.<field>`, since invariants read state fields directly (§14).
    Invariant {
        name: String,
        fields: HashSet<String>,
    },
    /// v0.116 (testing track slice 4): a `transition` predicate. Carries the JS
    /// names bound to the contextual `old` and `new` state records. The Bynk
    /// identifiers `old`/`new` lower to these (`new` is a JS reserved word, so
    /// both are renamed), and field access `old.<field>` reads off the `old`
    /// record.
    Transition { old: String, new: String },
    /// A service handler body (`emit_service`).
    ServiceHandler {
        handler: HandlerShared,
        /// v0.47: the `by` binder whose `.identity` is threaded through `deps`
        /// (so `<binder>.identity` lowers to `deps.identity` rather than the
        /// unit-value `undefined`).
        deps_identity_binder: Option<String>,
        /// v0.52: when lowering a multi-actor sum handler body, the `by` binder
        /// that names the resolved-actor value (threaded through `deps`, so the
        /// binder ident lowers to `deps.who` — the tagged union the body
        /// `match`es).
        actor_sum_binder: Option<String>,
    },
    /// A composed provider's operation body (`emit_provider`).
    ProviderOp { handler: HandlerShared },
    /// An agent handler body (`emit_agent`).
    AgentHandler {
        handler: HandlerShared,
        /// True when lowering an agent handler body. Used to rewrite
        /// `self.<keyField>` access into the appropriate local.
        in_agent_handler: bool,
        /// The name of the agent's `key id` field (so `self.<id>` resolves).
        agent_key_field: Option<String>,
        /// The `store`-agent working-record state, when the hosting agent is a
        /// `store` agent. Boxed: it is by far the largest payload in this enum,
        /// and every other body kind would otherwise pay for it.
        store: Option<Box<AgentStoreState>>,
    },
    /// A websocket lifecycle method on the hosting Durable Object
    /// (`emit_ws_do_method`).
    WsDoMethod {
        handler: HandlerShared,
        /// v0.47: as [`BodyMode::ServiceHandler::deps_identity_binder`].
        deps_identity_binder: Option<String>,
        /// As [`BodyMode::ServiceHandler::actor_sum_binder`]: a `from websocket`
        /// `on open` whose `by` is an actor sum reads the resolved actor from
        /// `deps.who`, which the edge forwarded and the DO's `fetch` rebuilt.
        actor_sum_binder: Option<String>,
        /// v0.104 (real-time track slice 3b): when lowering a `from websocket`
        /// `on open` body **into its hosting Durable Object** (the agent the
        /// upgrade transfers the connection to), the name of that agent. A
        /// transfer call `<Agent>(<key>).method(args)` whose `<Agent>` is this
        /// self-agent lowers to a direct `this.method(args, deps)` self-call
        /// rather than the cross-instance `__make<Agent>(key)` factory — the
        /// connection is already in this DO, so it never crosses an RPC boundary
        /// (DECISION A).
        ws_self_agent: Option<String>,
    },
    /// A `stub`/`where`/`requires` predicate value lowered via
    /// `lower_block_to_async_body` — test/property/contract scaffolding, never a
    /// real production provider body.
    PredicateScaffold { test: TestShared },
    /// A unit test `case` body (`lower_test_case_body`).
    TestCase {
        test: TestShared,
        /// v0.117 (testing track slice 5): the name of the recorded-call trace
        /// object (`__obs`), over which an observation (`Cap.op called …`) and
        /// `trace(Cap.op)` are lowered.
        observation_trace: Option<String>,
        /// v0.7: the target context's local service names. A `service.call(args)`
        /// or `service(args)` invocation where `service` is in this set lowers to
        /// `<service>.call(args, deps)` so the test wires its `deps` through.
        test_services: HashSet<String>,
        /// v0.182 (#664): the ordered handler kinds of each test service, so a
        /// cron (`svc.schedule("…")`) or queue (`svc.message(m)`) address can
        /// recover the position index the emitted key encodes (`cron_<svc>_<i>` /
        /// `queue_…`). http keys are a pure function of verb + path and need no
        /// lookup here. P6.37 (design/tracks/the-ir.md §6a): `IrHandlerKind`
        /// (P6.24a's own pure, unconditional mirror), not the raw AST
        /// `HandlerKind` this field used to store.
        test_service_handlers: HashMap<String, Vec<bynk_model::IrHandlerKind>>,
        /// I3 (#1971), #2020: the `(service, key)` handlers that bind an actor
        /// sum, whose `deps.who` a call-site principal builds. `key` is
        /// `"call"` for an `on call` handler and the emitted method name
        /// (`http_handler_method_name_ir`) for an http route.
        sum_handlers: HashSet<(String, String)>,
        /// #1994: the target has a handler that sends with `detach`, so `deps`
        /// carries the `__exec` double and each addressed service call is
        /// wrapped in `__bynkSettle(deps, …)`.
        settle_sends: bool,
    },
    /// An integration test `case` body (`lower_integration_case_body`).
    IntegrationCase {
        test: TestShared,
        /// v0.182 (Slice B, #667): the target's http service names. An http
        /// address on one of these lowers to a driver call
        /// (`__sysdrive_<svc>_<key>(args, sub)`) that drives a real
        /// `worker.fetch` with a signed credential, instead of the unit-tier
        /// direct handler call. Empty at the unit tier.
        system_http_services: std::collections::HashSet<String>,
        /// #707: the declared `(service, method, path)` http routes of the system
        /// target. A `(method, path)` call whose method is absent here but whose
        /// path is present is a **wrong-method** call — it drives the `405`
        /// fall-through through the generic `__sysdrive_wrongmethod_<svc>` driver.
        system_http_routes: std::collections::HashSet<(String, String, String)>,
        /// #708: for each declared `(service, method, path)` route that has a
        /// body param, the body's zero-based position among the route's
        /// positional call args (i.e. within `args[1..]`, matching handler-param
        /// declaration order) and its declared type. The raw driver
        /// (`__sysdrive_raw_*`, Slice C) forwards every slot as a `string`; a
        /// `Wire(…)` arg already lowers to that raw string, but a *typed* arg
        /// mixed into the same call must be converted: the body slot serialises
        /// through the same wire codec the typed driver uses
        /// (`JSON.stringify(serialise_expr_via(...))`), any other (path) slot
        /// just coerces via `String(...)`. Absent for a bodyless route.
        system_http_route_body:
            HashMap<(String, String, String), (usize, bynk_syntax::ast::TypeRef)>,
        /// #708: the type namespace (`<target>.`) `serialise_expr_via` needs to
        /// resolve a body param's custom codec when converting a typed arg for
        /// the raw driver. Mirrors the `type_ns` `emit_system_http_support`
        /// computes from the same suite target.
        system_http_type_ns: String,
        /// #2019: the refinement-guarded routes and the claims each
        /// refinement actor signs, for a call acting as a refinement.
        system_http_claims: Box<SystemHttpClaims>,
        /// #2136: the target's cron and queue services, driven at `system`
        /// through the Worker's `scheduled` / `queue` entry points.
        system_jobs: SystemJobServices,
    },
}

/// #2136: the cron and queue services a `system`-tier case drives through the
/// participant Worker's real `scheduled` and `queue` entry points. A
/// `svc.schedule("<expr>", …)` address on a cron service here lowers to
/// `__sysdrive_schedule_<svc>_<index>(…)`, and `q.message(m)` on a queue
/// service to `__sysdrive_message_<q>(m)` (`tests_emit::emit_system_job_support`).
#[derive(Debug, Clone, Default)]
pub struct SystemJobServices {
    /// Each cron service's schedule expressions, in declaration order (the
    /// position is the handler's `cron_<svc>_<index>` index).
    pub cron: HashMap<String, Vec<String>>,
    /// The queue services.
    pub queue: HashSet<String>,
    /// #3328: the email services, driven as raw MIME through the Worker's
    /// `email()` entry (`__sysdrive_message_<svc>(mail)`).
    pub email: HashSet<String>,
}

/// #2019: what a `system`-tier call needs to sign the claims a refinement
/// actor's predicate checks (`actor Admin = User where hasClaim("admin")`).
/// A route guarded by a refinement has drivers that take a trailing claims
/// object, signed into the JWT next to `sub`; a call acting as a refinement
/// actor passes that actor's witness claims, and any other call passes `{}`,
/// so the Worker's seam answers `403` exactly as it would a real token
/// lacking the claim.
#[derive(Debug, Clone, Default)]
pub struct SystemHttpClaims {
    /// The `(service, method, path)` routes whose `by` actor is a refinement.
    pub routes: HashSet<(String, String, String)>,
    /// Each refinement actor's witness claims, as a JavaScript object literal
    /// (`bynk_check::actors::claim_witness_js`).
    pub actors: HashMap<String, String>,
    /// #2155: each actor-sum route with a credentialed member, and its
    /// primary member — the actor a call with no principal acts as. Its
    /// signing drivers take the acting actor's name.
    pub sum_routes: HashMap<(String, String, String), String>,
    /// #3052: the `Oidc`-secured routes. Their valid-token drivers take the
    /// claims to mint into the token, from the call-site principal's
    /// `TokenClaims.of` lines (else none).
    pub oidc_routes: HashSet<(String, String, String)>,
}

/// #1750: the slot and label a statement-form value expression's tails assign
/// and break to. See [`LowerCtx::tail_slot`].
#[derive(Clone)]
pub(crate) struct TailSlot {
    pub(crate) slot: String,
    pub(crate) label: String,
}

thread_local! {
    /// #2620: the `.bynk` source being emitted and its project-relative identity
    /// path, so a `unreachable(…)` / `todo(…)` lowering can name its own
    /// `path:line:col`. Lowering reaches every body kind through a `LowerCtx`
    /// built at many sites; this is set once per unit by [`emit_project`].
    static FAULT_SOURCE: std::cell::RefCell<Option<(String, String)>> =
        const { std::cell::RefCell::new(None) };
}

/// Installs [`FAULT_SOURCE`] for the unit being emitted and restores the
/// previous value on drop.
pub(super) struct FaultSourceGuard(Option<(String, String)>);

impl FaultSourceGuard {
    pub(super) fn set(source: &str, identity_path: &str) -> Self {
        let next = Some((source.to_string(), identity_path.replace('\\', "/")));
        Self(FAULT_SOURCE.with(|s| s.replace(next)))
    }
}

impl Drop for FaultSourceGuard {
    fn drop(&mut self) {
        FAULT_SOURCE.with(|s| *s.borrow_mut() = self.0.take());
    }
}

/// #2620: the `path:line:col` of a byte offset in the unit being emitted, or
/// `None` outside [`emit_project`] (the single-file path has no identity).
pub(crate) fn fault_source_location(offset: usize) -> Option<String> {
    FAULT_SOURCE.with(|s| {
        s.borrow().as_ref().map(|(source, path)| {
            let (line, col) = bynk_syntax::span::line_col(source, offset);
            format!("{path}:{line}:{col}")
        })
    })
}

/// v0.59: the source context an `assert` lowering needs to turn its span into a
/// `path:line:col` location. Owned (cloned once per test-case body) to keep the
/// lowering free of extra lifetime threading; test-file sources are small and
/// this is compile-time only.
#[derive(Clone)]
pub(crate) struct AssertLoc {
    pub source: String,
    pub rel_path: String,
}
