//! Which `runtime.ts` helpers an emitted module actually references.
//!
//! Most of the runtime import list is fixed (`Ok`, `Err`, `Some`, `None`, …),
//! but four groups are conditional, because importing them unconditionally
//! would put names in every module that most modules never use:
//!
//! - the `Bytes` helpers (`__bynkBytesEqual` and friends, v0.110 / ADR 0142),
//!   emitted by the `Bytes` kernel, `==` on `Bytes`, and the boundary codecs;
//! - the structural-equality walker (`__bynkEq`, #1652), emitted by `==`/`!=`
//!   on any operand that is neither a primitive nor `Bytes`;
//! - the ICU formatters (`selectPluralArm`, `formatIcuNumber`, `formatIcuDate`,
//!   #878), emitted by a `messages` bundle's `render`;
//! - the boundary-codec group (`Ok`, `Err`, `Result`, `BoundaryError`, #914),
//!   which an *inlined* deserialiser builds directly;
//! - the JSON-codec group (`Ok`, `Err`, `Result`, `JsonValue`, `JsonError`,
//!   #914), which the `Json.decode[T]` wrapper names in its own signature and
//!   body, independently of which arm the inner deserialiser takes.
//!
//! Most modules import `Ok`/`Err`/`Result` unconditionally and never consult the
//! last two; they exist for the module kinds that curate their runtime import
//! list — a Worker's `compose.ts` and the test-scaffold modules.
//!
//! #917 adds a fifth, differently-shaped piece of state, riding this same
//! shared reference for the same reason: a test-scaffold module's `Json.decode`/
//! `Json.encode` on a named type has no local codec to delegate to (the unit it
//! imports exports the type but not a codec for it), so the module generates its
//! own — the `json_codec_qual`/`json_codec_roots` fields carry the namespace a
//! foreign type's TS positions render through and the roots its bodies reached
//! for, respectively. Not a runtime-*helper* reference like the four flags
//! above, but the same "note during lowering, decide after" shape.
//!
//! The condition is "did emission actually reference it", which is a fact only
//! emission knows. This type is how that fact travels: the producers call
//! [`RuntimeUse::note_bytes`] / [`RuntimeUse::note_icu`] as they emit, and the
//! header/post-pass reads the flags to decide the import list.
//!
//! # Why not scan the generated text
//!
//! This replaces `out.contains("__bynkBytes")` and friends. Scanning the output
//! for the helper's own name is wrong in both directions. It over-matches: the
//! generated text also carries user-authored string literals, doc comments, and
//! field names, any of which could contain the marker and pull in an import
//! nothing calls. Worse, it under-matches: `out.contains("selectPluralArm(")`
//! quietly depends on the call being emitted with no space before its paren, so
//! a formatting change in an unrelated part of the emitter silently drops a
//! required import and produces a module that does not compile. There is no test
//! that would obviously catch it, because the marker and the scan live in
//! different files.
//!
//! The same reasoning is already recorded a few lines from the code this
//! replaces: `HttpResult`'s import is decided structurally (over the AST)
//! "so a comment or string literal mentioning `HttpResult` never triggers a
//! spurious import" (`file_mentions_http_result`). This extends that principle
//! to the two conditions that were still text scans.
//!
//! # What keeps the producers honest
//!
//! A producer added without a matching `note_*` call emits a module that
//! references an unimported name. Deliberately *not* guarded here by comparing
//! the flags against a scan of the generated text: that scan would inherit the
//! over-matching described above, so a user string literal containing the marker
//! would fire the assertion on valid input — trading a rare missing import for a
//! panic on legitimate code. The check belongs where it is already total:
//! `bynkc`'s fixture suite type-checks every emitted fixture under `tsc --strict`
//! (`tests/tsc_verify.rs`), which fails loudly on an unimported reference.
//!
//! Interior mutability (`Cell`) rather than `&mut`: the producers sit at the
//! bottom of a deep tree of `&`-taking emit functions — several of them pure
//! `String`-returning helpers — and a shared `&RuntimeUse` threads through that
//! tree without turning every intermediate signature into a `&mut` chain.
//!
//! The consequence, recorded here so it does not have to be bisected for later:
//! `Cell` is `!Sync`, so [`RuntimeUse`] is `!Sync`, and so is `EmitProjectCtx`
//! now that it owns one — an `&EmitProjectCtx` can no longer cross a thread
//! boundary. Nothing in the tree is parallel today, so this costs nothing, but
//! per-unit emission is the obvious thing to parallelise, and that is when it
//! will bite. Swapping the fields to `AtomicBool` with `Relaxed` ordering
//! restores `Sync` at no meaningful cost and leaves every call site reading the
//! same — do that rather than unpicking the shared-`&` design.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use bynk_check::checker::TyId;

/// Per-module accumulator of conditional runtime-helper references.
///
/// One instance per emitted module (it lives on that module's emission context),
/// so flags never leak between files.
#[derive(Default)]
pub(crate) struct RuntimeUse {
    bytes: Cell<bool>,
    /// #2016: the module references the `Bytes` operation helpers (`concat`,
    /// `compare`, hex, base64url), a group apart from `bytes` so a module that
    /// only builds or compares `Bytes` keeps its import list.
    bytes_ops: Cell<bool>,
    /// K3 (#2592): the module references the `Instant` ISO-8601 helpers.
    time_ops: Cell<bool>,
    /// #2640: the module references the `par` block's join, `__bynkSettleAll`.
    settle_all: Cell<bool>,
    /// #2683: the module references `e.within(d)`'s race, `__bynkWithin`.
    within: Cell<bool>,
    /// #2711: the module references `a.race(b)`'s helper, `__bynkRace`.
    race: Cell<bool>,
    /// #2683: the test module runs on virtual time, so an addressed call is
    /// driven by `__bynkSettleVirtual`.
    virtual_time: Cell<bool>,
    /// K2 (#2594): the module references the percent-encoding helpers.
    url_ops: Cell<bool>,
    /// #2341 (L4): the module builds a `RequestHeaders` (`__requestHeaders`).
    request_headers: Cell<bool>,
    /// #2341 (L4): a system-tier driver sends a test's headers
    /// (`__requestHeaderPairs`).
    request_header_pairs: Cell<bool>,
    /// #2380 (L5): the module builds a `TokenClaims` (`__tokenClaims`).
    token_claims: Cell<bool>,
    /// #1652: the module references `__bynkEq` (structural `==`).
    eq: Cell<bool>,
    /// #1657: the module references the `Int` domain traps (`__bynkIntDiv`,
    /// `__bynkToInt`).
    int: Cell<bool>,
    /// Constrained generics slice 1: the comparator helpers (`__bynkCmpInt`,
    /// `__bynkCmpRecord`, …) the module's witnesses, ordering operators and
    /// ordering kernels reference.
    cmp: RefCell<std::collections::BTreeSet<&'static str>>,
    icu: Cell<bool>,
    boundary_codec: Cell<bool>,
    json_codec: Cell<bool>,
    /// #1934: the module constructs an `HttpResult` variant. A source module
    /// imports `HttpResult` by its own structural scan; a test module, whose
    /// case compares a typed outcome with `Ok(…)`, reads this.
    http_result: Cell<bool>,
    /// #917: for a test-scaffold module, the type-only namespace each
    /// codec-eligible named type reaches through (`"Order" -> "orders."`) — the
    /// module has no local declaration of its own, only a namespace import of
    /// the suite's target (and its `uses`). Empty (the default) on every other
    /// emission path, where every name renders bare.
    json_codec_qual: RefCell<HashMap<String, String>>,
    /// #917: the `Json.decode[T]`/`Json.encode` target types a test-scaffold
    /// module's case/property/stub bodies actually reached for, collected as they
    /// lower. `emit_test_module` drains this once all bodies are lowered to
    /// compute the codec closure those bodies need — the unit module they import
    /// exports the type but no codec, so a test-scaffold module generates its own
    /// (#661's caller-generates-its-own-codec pattern, applied to test scaffolding).
    /// P6.28 (design/tracks/the-ir.md §6a): stored as the checker's own resolved
    /// `TyId` rather than a re-derived `TypeRef` — both push sites already hold
    /// the `TyId` before converting it for their own, separate codec-rendering
    /// use, so this field no longer needs its own `TypeRef` conversion at push
    /// time; the drain site converts once, immediately before the one consumer
    /// (`collect_codec_closure`) that is still genuinely `TypeRef`-driven.
    json_codec_roots: RefCell<Vec<TyId>>,
    /// #2052: the target's `from Events(E)` services a test-scaffold module's
    /// bodies deliver an event to (`svc.event(e)`). `emit_test_module` emits
    /// one `__bynkEvent_<svc>` delivery helper for each, once every body has
    /// lowered, so a module that delivers none is unchanged.
    test_event_services: RefCell<std::collections::BTreeSet<String>>,
}

impl RuntimeUse {
    /// Record that the module references the `Bytes` runtime helpers.
    pub fn note_bytes(&self) {
        self.bytes.set(true);
    }

    /// Record that the module references the `Bytes` operation helpers (#2016).
    pub fn note_bytes_ops(&self) {
        self.bytes_ops.set(true);
    }

    /// Record that the module references the `Instant` ISO-8601 helpers (#2592).
    pub fn note_time_ops(&self) {
        self.time_ops.set(true);
    }

    /// Record that the module references the percent-encoding helpers (#2594).
    pub fn note_url_ops(&self) {
        self.url_ops.set(true);
    }

    /// Record that the module references the `par` join (#2640).
    pub fn note_settle_all(&self) {
        self.settle_all.set(true);
    }

    /// Record that the test module runs on virtual time (#2683).
    pub fn note_virtual_time(&self) {
        self.virtual_time.set(true);
    }

    /// Whether the test module runs on virtual time (#2683).
    pub fn virtual_time(&self) -> bool {
        self.virtual_time.get()
    }

    /// Record that the module references the `within` race (#2683).
    pub fn note_within(&self) {
        self.within.set(true);
    }

    /// Record that the module references the `race` helper (#2711).
    pub fn note_race(&self) {
        self.race.set(true);
    }

    /// Record that the module builds a `RequestHeaders` value (#2341).
    pub fn note_request_headers(&self) {
        self.request_headers.set(true);
    }

    /// Record that a system-tier driver sends a test's headers (#2341).
    pub fn note_request_header_pairs(&self) {
        self.request_header_pairs.set(true);
    }

    /// Whether `__requestHeaderPairs` must be imported (#2341).
    pub fn request_header_pairs(&self) -> bool {
        self.request_header_pairs.get()
    }

    /// Record that the module builds a `TokenClaims` value (#2380).
    pub fn note_token_claims(&self) {
        self.token_claims.set(true);
    }

    /// Whether `__tokenClaims` must be imported (#2380).
    pub fn token_claims(&self) -> bool {
        self.token_claims.get()
    }

    /// Whether `__requestHeaders` must be imported (#2341).
    pub fn request_headers(&self) -> bool {
        self.request_headers.get()
    }

    /// Record that the module references `__bynkEq`, the structural-equality
    /// walker behind `==`/`!=` on a non-primitive operand (#1652).
    pub fn note_eq(&self) {
        self.eq.set(true);
    }

    /// Record that the module constructs an `HttpResult` variant (#1934).
    pub fn note_http_result(&self) {
        self.http_result.set(true);
    }

    /// Whether a test module must import `HttpResult` (#1934).
    pub fn http_result(&self) -> bool {
        self.http_result.get()
    }

    /// Record that the module references the `Int` domain traps: `Int` division
    /// (`__bynkIntDiv`) or a Float→`Int` conversion (`__bynkToInt`), #1657.
    pub fn note_int(&self) {
        self.int.set(true);
    }

    /// Record that the module references the comparator helper `name`
    /// (`compare.ts`; constrained-generics slice 1).
    pub fn note_cmp(&self, name: &'static str) {
        self.cmp.borrow_mut().insert(name);
    }

    /// Record that the module references the key helper `name` (`keys.ts`;
    /// constrained-generics slice 2). They share the comparator group's import
    /// run, which is a generic set of runtime names.
    pub fn note_key(&self, name: &'static str) {
        self.note_cmp(name);
    }

    /// The comparator helpers to import, as a `", a, b"` run (empty when the
    /// module uses none), in the shape the other conditional groups take.
    pub fn cmp_imports(&self) -> String {
        self.cmp
            .borrow()
            .iter()
            .map(|n| format!(", {n}"))
            .collect::<String>()
    }

    /// Record that the module references the ICU formatting helpers.
    pub fn note_icu(&self) {
        self.icu.set(true);
    }

    /// Record that the module inlines a boundary deserialiser, which builds
    /// `Ok(…)` / `Err(… as BoundaryError)` directly rather than delegating to a
    /// `deserialise_<T>` in its own namespace.
    pub fn note_boundary_codec(&self) {
        self.boundary_codec.set(true);
    }

    /// Record that the module emits the `Json.decode[T]` wrapper, whose
    /// signature and body name `Result`, `JsonValue` and `JsonError` regardless
    /// of which arm the inner deserialiser takes.
    pub fn note_json_codec(&self) {
        self.json_codec.set(true);
    }

    /// #917: set once, up front, before any body lowers — the type-only
    /// namespace qualifier a test-scaffold module's codec calls render foreign
    /// named types through. A no-op default (empty map) everywhere else.
    pub fn set_json_codec_qual(&self, qual: HashMap<String, String>) {
        *self.json_codec_qual.borrow_mut() = qual;
    }

    /// The namespace qualifier a test-scaffold module's codec calls render
    /// foreign named types through (empty on every other emission path).
    pub fn json_codec_qual(&self) -> std::cell::Ref<'_, HashMap<String, String>> {
        self.json_codec_qual.borrow()
    }

    /// #917: record a `Json.decode[T]`/`Json.encode` target type a test-scaffold
    /// body reached for, so the module that spliced it in can generate the codec
    /// helper its delegating call needs.
    pub fn note_json_codec_root(&self, t: TyId) {
        self.json_codec_roots.borrow_mut().push(t);
    }

    /// #2052: record that a test-scaffold body delivers an event to `service`.
    pub fn note_test_event_service(&self, service: &str) {
        self.test_event_services
            .borrow_mut()
            .insert(service.to_string());
    }

    /// #2052: the services a test-scaffold module delivers events to, sorted.
    pub fn test_event_services(&self) -> Vec<String> {
        self.test_event_services.borrow().iter().cloned().collect()
    }

    /// Drain the accumulated `Json` codec roots — called once, after every
    /// case/property/stub body in the module has lowered.
    pub fn take_json_codec_roots(&self) -> Vec<TyId> {
        std::mem::take(&mut *self.json_codec_roots.borrow_mut())
    }

    /// Whether the `Bytes` helpers must be imported.
    pub fn bytes(&self) -> bool {
        self.bytes.get()
    }

    /// Whether the `Bytes` operation helpers must be imported (#2016).
    pub fn bytes_ops(&self) -> bool {
        self.bytes_ops.get()
    }

    /// Whether the `Instant` ISO-8601 helpers must be imported (#2592).
    pub fn time_ops(&self) -> bool {
        self.time_ops.get()
    }

    /// Whether the percent-encoding helpers must be imported (#2594).
    pub fn url_ops(&self) -> bool {
        self.url_ops.get()
    }

    /// Whether `__bynkWithin` must be imported (#2683).
    pub fn within(&self) -> bool {
        self.within.get()
    }

    /// Whether `__bynkRace` must be imported (#2711).
    pub fn race(&self) -> bool {
        self.race.get()
    }

    /// Whether `__bynkSettleAll` must be imported (#2640).
    pub fn settle_all(&self) -> bool {
        self.settle_all.get()
    }

    /// Whether `__bynkEq` must be imported (#1652).
    pub fn eq(&self) -> bool {
        self.eq.get()
    }

    /// Whether the `Int` domain traps must be imported (#1657).
    pub fn int(&self) -> bool {
        self.int.get()
    }

    /// Whether the `Ok` / `Err` / `Result` / `BoundaryError` group must be
    /// imported.
    pub fn boundary_codec(&self) -> bool {
        self.boundary_codec.get()
    }

    /// Whether the `Ok` / `Err` / `Result` / `JsonValue` / `JsonError` group
    /// must be imported.
    pub fn json_codec(&self) -> bool {
        self.json_codec.get()
    }

    /// Whether the ICU formatting helpers must be imported.
    pub fn icu(&self) -> bool {
        self.icu.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The type's contract, written down: the flags are independent and a
    /// note latches. Everything else about this type is `Cell` behaviour and is
    /// not worth a test — the coverage that matters drives the real producers
    /// end-to-end (`emitter::conditional_runtime_import_tests`).
    #[test]
    fn notes_are_independent_and_latch() {
        let u = RuntimeUse::default();
        u.note_bytes();
        assert!(u.bytes());
        assert!(!u.icu(), "noting one helper does not set the other");
        u.note_bytes();
        assert!(u.bytes(), "a second note is not a toggle");
    }
}
