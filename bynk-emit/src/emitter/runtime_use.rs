//! Which runtime helpers an emitted module's *callers other than its own header* need.
//!
//! Every module the emitter or the harness builds takes its runtime import from its built
//! tree: a unit module since 1A.36b-1 (`runtime_header`), the Worker `index.ts` and
//! `compose.ts` since 1A.36b-2, and the generated test modules since 1B.5e. **So nothing here
//! decides an import at all.** What remains is emission itself: `plain_json`, `virtual_time`,
//! `http_result_cookie` (the `bynk` `Cookie` type a case reads, not a runtime name) and the
//! codec roots decide what is *emitted*. 1B.5e deleted the 28 flags whose only reader was the
//! test harness's predicted import list, with their notes and fields.
//!
//! A Worker module's own `RuntimeUse` (`workers.rs`, `workers_entry.rs`) is a sink: the codec
//! producers shared with the unit modules take one to note into, and nothing reads it back.
//!
//! The notes that remain are the ones emission itself reads: the `Bytes` helpers
//! (`__bynkBytesEqual` and friends, v0.110 / ADR 0142), the `detach` reporter, the comparator,
//! key and `Decimal` helper sets a module's witnesses name, and the codec state below. The
//! notes whose only reader was an emitter-built module's header (the ICU formatters, the
//! session-cookie helpers and the two trace helpers) went in 1A.36b-1; those whose only
//! reader was the test harness's import list went in 1B.5e.
//!
//! #917 adds a differently-shaped piece of state, riding this same shared reference for
//! the same reason: a test-scaffold module's `Json.decode`/`Json.encode` on a named type has
//! no local codec to delegate to (the unit it imports exports the type but not a codec for
//! it), so the module generates its own — the `json_codec_qual`/`json_codec_roots` fields
//! carry the namespace a foreign type's TS positions render through and the roots its bodies
//! reached for, respectively.
//!
//! The condition is "did emission actually reference it", which is a fact only
//! emission knows. This type is how that fact travels: the producers call
//! [`RuntimeUse::note_bytes`] and the like as they emit, and the emission that depends on the
//! fact reads it afterwards.
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
//! Every header goes further: it reads the tree, which carries each name as a node, so no
//! note is needed for an import at all.
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

use super::internal_fault::InternalFault;

/// Per-module accumulator of conditional runtime-helper references.
///
/// One instance per emitted module (it lives on that module's emission context),
/// so flags never leak between files.
#[derive(Default)]
pub struct RuntimeUse {
    bytes: Cell<bool>,
    /// #3437: the module references `e.detach()`'s reporter, `__bynkDetach`.
    detach: Cell<bool>,
    /// #2683: the test module runs on virtual time, so an addressed call is
    /// driven by `__bynkSettleVirtual`.
    virtual_time: Cell<bool>,
    /// #3277: a case reads the cookies of an `HttpResult`, so names the `bynk` `Cookie` type.
    http_result_cookie: Cell<bool>,
    /// Constrained generics slice 1: the comparator helpers (`__bynkCmpInt`,
    /// `__bynkCmpRecord`, …) the module's witnesses, ordering operators and
    /// ordering kernels reference.
    cmp: RefCell<std::collections::BTreeSet<&'static str>>,
    /// #2858: the project has a plain-JSON route, so the generated `Option`
    /// encoders carry the plain branch `withPlainJson` switches on. Absent, they
    /// are emitted byte-for-byte as before.
    plain_json: Cell<bool>,
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
    /// 1A.34: the checker facts lowering needed and did not find, in the order
    /// they were met ([`super::internal_fault`]).
    faults: RefCell<Vec<InternalFault>>,
}

impl RuntimeUse {
    /// Record that the project has a plain-JSON route (#2858).
    pub fn set_plain_json(&self, on: bool) {
        self.plain_json.set(on);
    }

    /// Whether the `Option` encoders carry the plain-JSON branch (#2858).
    pub fn plain_json(&self) -> bool {
        self.plain_json.get()
    }

    /// Record that the module references the `Bytes` runtime helpers.
    pub fn note_bytes(&self) {
        self.bytes.set(true);
    }

    /// Record that the test module runs on virtual time (#2683).
    pub fn note_virtual_time(&self) {
        self.virtual_time.set(true);
    }

    /// Whether the test module runs on virtual time (#2683).
    pub fn virtual_time(&self) -> bool {
        self.virtual_time.get()
    }

    /// Record that the module references the `detach` reporter (#3437).
    pub fn note_detach(&self) {
        self.detach.set(true);
    }

    /// Record that a result method names the `bynk` `Cookie` type (#3277).
    pub fn note_http_result_cookie(&self) {
        self.http_result_cookie.set(true);
    }

    /// Whether the module must import the `Cookie` type (#3277).
    pub fn http_result_cookie(&self) -> bool {
        self.http_result_cookie.get()
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

    /// Record that the module references the `Decimal` runtime helper `name`
    /// (`decimal.ts`, #2622). Rides the same set as the comparator helpers, so
    /// every import site already emits it.
    pub fn note_decimal(&self, name: &'static str) {
        self.note_cmp(name);
    }

    /// Record that the module references the `Instant` wire helper `name`
    /// (`time.ts`, #3974). Rides the comparator group's import run too.
    pub fn note_time(&self, name: &'static str) {
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

    /// #3111: how many codec roots have been noted so far; a runner records it
    /// before lowering and reads back what it added with
    /// [`json_codec_roots_since`](Self::json_codec_roots_since).
    pub fn json_codec_roots_len(&self) -> usize {
        self.json_codec_roots.borrow().len()
    }

    /// #3111: the codec roots noted at or after `mark` (not drained).
    pub fn json_codec_roots_since(&self, mark: usize) -> Vec<TyId> {
        self.json_codec_roots
            .borrow()
            .get(mark..)
            .unwrap_or_default()
            .to_vec()
    }

    /// 1A.34: record a checker fact lowering needed and did not find.
    pub fn note_internal_fault(&self, span: bynk_syntax::span::Span, what: String) {
        self.faults.borrow_mut().push(InternalFault { span, what });
    }

    /// 1A.34: drain the recorded faults, once the unit has been lowered. Empty
    /// for every program the checker passed.
    pub fn take_faults(&self) -> Vec<InternalFault> {
        std::mem::take(&mut *self.faults.borrow_mut())
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

    /// Whether `__bynkDetach` must be imported (#3437).
    pub fn detach(&self) -> bool {
        self.detach.get()
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
        assert!(!u.detach(), "noting one helper does not set the other");
        u.note_bytes();
        assert!(u.bytes(), "a second note is not a toggle");
    }
}
