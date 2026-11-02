//! P8.4 (#1515): a durable path↔`FileId` interning table plus one shared,
//! content-keyed parse cache — settled by ADR 0413 as the fix for R3.13's
//! own file level. Replaces `bynk-ide`'s `PROJECT_UNIT_CACHE`
//! (`bynk-check` cannot depend on `bynk-ide`, so the cache the diagnostics
//! path (`bynk_check::analysis::analyse_project` → `phase_parse` →
//! [`crate::discovery::parse_sources`]) actually needs has to live here, one
//! layer down) and gives `bynk_check::project_model::phase_parse` a `FileId`
//! that survives across separate analysis calls instead of resetting to
//! zero on every one ([DECISION B]).
//!
//! [DECISION A] (ADR 0413): one cache, `Ast(FileId)`, not a separate
//! `Tokens(FileId)` cache — neither consumer this slice migrates reads a
//! token stream independently of the parse it feeds.
//!
//! [DECISION B]: the interning table is `path ↔ FileId`
//! (`HashMap<PathBuf, FileId>`, no `index_vec` crate — matching P8.1's own
//! "no new indexing infrastructure" posture); the parse cache keys on the
//! interned `FileId` and invalidates by content equality, mirroring
//! `PROJECT_UNIT_CACHE`'s own proven scheme exactly.
//!
//! [DECISION C]: one slice, one cache. `PROJECT_UNIT_CACHE` is deleted
//! (`bynk-ide/src/completion.rs`), not left running alongside this one —
//! two independently-invalidated caches of the same fact is the exact "no
//! fact in two hand-synced copies" defect this trajectory's phase 1 already
//! named as a standing invariant.
//!
//! [DECISION D] (new — neither the issue nor ADR 0413 examined this):
//! **`ExprId` must be durably allocated too, from a counter that never
//! resets, for the same reason `FileId` must be.** `parser::
//! parse_units_with_warnings_from`'s own doc comment names the hazard this
//! closes one level up: a multi-file commons merges sibling files' methods
//! into one `check_record` call, and two independently zero-based files
//! would collide on the same `ExprId` in the same `expr_types` map
//! (`collect_unit_methods`, caught live by finding #28). That hazard is
//! usually avoided by threading one counter across every file **within a
//! single `phase_parse` call**. Caching a file's parsed `SourceUnit`
//! *across calls* reopens it one level up: if call 2 serves file A from
//! cache (keeping its `ExprId`s from call 1's counter position) while
//! freshly parsing changed file B from a counter that started over at 0,
//! A's and B's `ExprId`s collide in call 2's own `expr_types` map — the
//! identical defect class, now triggered by caching rather than by two
//! files in one call. Fixed the same way `FileId` is: `next_expr_id` lives
//! in this module's own durable state, advanced only on an actual parse
//! (a cache hit consumes no new ids, since the cached `SourceUnit`'s own
//! ids are already fixed), never reset. Global uniqueness across the whole
//! process trivially implies uniqueness within any one call, so this is a
//! strict strengthening of the existing guarantee, not a new one.
//!
//! [DECISION E] (new): this cache stores the **strict** parse
//! (`parser::parse_units_with_warnings_from`, `recover_mode: false`) — the
//! one the build/diagnostics path needs, since a build must never silently
//! succeed on broken syntax by reading a best-effort recovered AST.
//! #1710 amends this without weakening it: when the strict parse fails in the
//! parser, the entry *also* keeps the recovering parse of the same tokens
//! (`parser::parse_units_recovering_from`, ids from the same durable counter),
//! read through [`cached_recovery`] by the project path so a file's surviving
//! declarations are still checked. The strict result is unchanged and still
//! the `Err` the build reads; the recovered units are for diagnostics only. `bynk
//! -ide::completion`'s own recovery-tolerant parsing
//! (`parser::parse_unit_with_recovery`) is a genuinely different parser
//! configuration, not just a different entry point over the same result —
//! for syntactically **clean** source the two produce the identical AST
//! (there is nothing to recover from), so completion reads this cache
//! directly for the common case; only when the cached/fresh strict result
//! actually carries errors does completion fall back to its own local,
//! uncached recovery-parse for that one file (`bynk_ide::completion`'s own
//! `parse_source_unit`, calling `parser::parse_unit_with_recovery` directly
//! — not part of this crate) — the rare case (another project file mid-edit
//! elsewhere, not the buffer under the cursor, which this cache was never
//! in the path for to begin with) traded for never caching two different
//! parser configurations' output under one key.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use bynk_syntax::ast::SourceUnit;
use bynk_syntax::error::CompileError;
use bynk_syntax::span::FileId;
use bynk_syntax::{lexer, parser};

/// A strict parse's own `Result` shape — the file's units plus any
/// warnings on success, or the hard errors on failure — each side
/// `Arc`-wrapped so a cache hit clones cheaply.
type StrictParseResult =
    Result<(Arc<Vec<SourceUnit>>, Arc<Vec<CompileError>>), Arc<Vec<CompileError>>>;

/// One file's cached strict-parse result, tagged with the exact content
/// string it was parsed from — the same "compare the whole content, not a
/// timestamp" invalidation `PROJECT_UNIT_CACHE` already trusted.
struct CachedParse {
    content: Arc<str>,
    result: StrictParseResult,
    /// #1710: when the strict parse failed in the *parser* (the source
    /// lexed), the recovering parse of the same tokens, its ids drawn from the
    /// same durable counter — for diagnostics only ([DECISION E]).
    recovered: Option<Arc<parser::Recovered>>,
}

#[derive(Default)]
struct ParseCacheState {
    next_file_id: u32,
    file_ids: HashMap<PathBuf, FileId>,
    /// [DECISION D]: durable, never reset — see this module's own doc
    /// comment. Shares its `u32` space with `bynk_check::project_model`'s
    /// own first-party `ExprId` reservation
    /// (`FIRSTPARTY_ID_BASE = 1_000_000_000`, spaced `FIRSTPARTY_ID_BLOCK =
    /// 1_000_000` apart per unit) — that scheme was sized for a counter that
    /// reset to 0 every call, so this one growing forever (never resetting)
    /// could in principle reach it. In practice this needs on the order of a
    /// billion `ExprId`s consumed over one process's lifetime — many orders
    /// of magnitude past any real editing session — to become a real
    /// concern; noted here, not defended against, the same "generous
    /// headroom, revisit if it ever measurably matters" posture this
    /// codebase already applies to `PROJECT_UNIT_CACHE_CAP` and to
    /// `FIRSTPARTY_ID_BLOCK`'s own spacing.
    next_expr_id: u32,
    entries: HashMap<FileId, CachedParse>,
}

/// Cap on distinct cached files, mirroring `PROJECT_UNIT_CACHE_CAP`'s own
/// reasoning exactly: without one, a long-lived server hopping across many
/// workspaces accumulates one entry per path ever parsed, and a
/// renamed/deleted file leaves a dangling entry behind. Past the cap the
/// parse-result cache clears wholesale (entries repopulate lazily); the
/// interning table (`file_ids`) is left alone — a stable `FileId` for a
/// path already seen is cheap to keep and is exactly the durability this
/// slice exists to provide, so clearing it would defeat the point every
/// time the cap is hit.
const PARSE_CACHE_CAP: usize = 4096;

/// Must match `bynk_check::project_model::FIRSTPARTY_ID_BASE` exactly.
/// `bynk-project` cannot depend on `bynk-check` (the crate graph runs the
/// other way), so this is a duplicated, keep-in-sync value — guarded by the
/// `debug_assert!` in `cached_parse_in`, not by the type system. PR
/// #1520's own bot review (finding #3): a durable counter that quietly
/// crossed this boundary would silently alias a project file's `ExprId`s
/// onto a first-party unit's own reserved block — wrong types, wrong
/// diagnostics, sharing one `expr_types` map, with nothing to point at. In
/// practice this needs on the order of a billion `ExprId`s consumed over one
/// process's lifetime to become reachable; the assertion exists so crossing
/// it is loud, not because it is expected to fire.
const FIRSTPARTY_ID_BASE: u32 = 1_000_000_000;

static CACHE: LazyLock<Mutex<ParseCacheState>> =
    LazyLock::new(|| Mutex::new(ParseCacheState::default()));

/// The durable `FileId` for `path` — assigned once, on first use, and
/// stable for the life of the process from then on, even across content
/// edits to that same path (a `FileId` is a path identity, not a content
/// one; [`cached_parse`]'s own content-keyed cache is what tracks edits).
///
/// PR #1520's own bot review (finding #1): recovers from a poisoned lock
/// rather than propagating the poison — a parser panic on one input (a real,
/// reachable failure mode this codebase already handles at the wasm
/// boundary, `bynk-wasm/src/lib.rs`'s own `catch_unwind`) must not brick
/// every later, perfectly valid parse for the life of the process. Safe
/// specifically because this cache's own invariants — `file_ids` maps a
/// path to a stable id; `entries[id].content` is the content
/// `entries[id].result` was parsed from — both hold even mid-panic: a panic
/// during the parse call in `cached_parse_in` happens strictly before that
/// entry would have been inserted, so the state a panicked lock holder left
/// behind is never a half-written entry, only a possibly-stale (never
/// wrong) one.
pub fn file_id_for(path: &Path) -> FileId {
    let mut state = CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    file_id_for_locked(&mut state, path)
}

fn file_id_for_locked(state: &mut ParseCacheState, path: &Path) -> FileId {
    if let Some(&id) = state.file_ids.get(path) {
        return id;
    }
    let id = FileId(state.next_file_id);
    state.next_file_id += 1;
    state.file_ids.insert(path.to_path_buf(), id);
    id
}

/// The strict parse of `path`'s `content` — [`parser::parse_units_with_warnings_from`],
/// cached by content equality and keyed on the durable `FileId`
/// [`file_id_for`] assigns `path`. On a cache hit, no new `ExprId`s are
/// consumed (the cached units already carry their own, fixed at whenever
/// they were last actually parsed); on a miss, [DECISION D]'s durable
/// counter advances by however many the fresh parse used.
///
/// The lock is held across the parse itself (not released and re-acquired
/// the way `PROJECT_UNIT_CACHE` did around its own parse) — [DECISION D]'s
/// counter must be threaded through the parse call, and releasing the lock
/// mid-parse would need a pre-reserved `ExprId` block of unknown size.
/// Serialises concurrent parses process-wide; each individual file's parse
/// is fast enough (sub-millisecond, ordinarily) that this is a deliberate,
/// documented trade of a little concurrency for not having to invent a
/// block-reservation scheme — revisit only if profiling ever shows real
/// contention. See [`file_id_for`]'s own doc comment for why the lock's
/// `unwrap_or_else(PoisonError::into_inner)` here is safe.
pub fn cached_parse(path: &Path, content: &str) -> (FileId, StrictParseResult) {
    let mut state = CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cached_parse_in(&mut state, path, content, PARSE_CACHE_CAP)
}

/// [`cached_parse`]'s own core, parameterised over the cache state and its
/// eviction cap so a test can drive a small, locally-owned
/// `ParseCacheState` to exercise the eviction boundary precisely — PR
/// #1520's own bot review (finding #2): a test that instead forced the
/// real, global `CACHE` past its real, 4096-entry cap raced every other
/// test in this module asserting on that same process-wide `static`
/// (`cargo test` runs one binary's tests concurrently by default), and cost
/// thousands of real parses to do it.
fn cached_parse_in(
    state: &mut ParseCacheState,
    path: &Path,
    content: &str,
    cap: usize,
) -> (FileId, StrictParseResult) {
    let id = file_id_for_locked(state, path);
    if let Some(entry) = state.entries.get(&id)
        && &*entry.content == content
    {
        return (id, entry.result.clone());
    }

    let mut recovered = None;
    // #2807: a lexical error no longer ends the file's analysis. The lexer
    // recovers past it, so the parser (and, through `recovered`, the checker)
    // still see everything the lexer could read. The strict result is the
    // lexical errors then, the build's `Err`, as a syntax error's is.
    let (tokens, lex_errors) = lexer::tokenize_recovering_in(content, id);
    let result = if lex_errors.is_empty() {
        match parser::parse_units_with_warnings_from(&tokens, content, &mut state.next_expr_id) {
            Ok((units, warnings)) => Ok((Arc::new(units), Arc::new(warnings))),
            Err(errors) => {
                recovered = Some(Arc::new(parser::parse_units_recovering_from(
                    &tokens,
                    content,
                    &mut state.next_expr_id,
                )));
                Err(Arc::new(errors))
            }
        }
    } else {
        let mut rec =
            parser::parse_units_recovering_from(&tokens, content, &mut state.next_expr_id);
        rec.errors = lexer::without_lex_echoes(&lex_errors, &tokens, rec.errors);
        recovered = Some(Arc::new(rec));
        Err(Arc::new(lex_errors))
    };
    debug_assert!(
        state.next_expr_id < FIRSTPARTY_ID_BASE,
        "durable ExprId counter ({}) reached the first-party reservation ({FIRSTPARTY_ID_BASE}) \
         — see this module's own doc comment",
        state.next_expr_id,
    );

    if state.entries.len() >= cap && !state.entries.contains_key(&id) {
        state.entries.clear();
    }
    state.entries.insert(
        id,
        CachedParse {
            content: Arc::from(content),
            result: result.clone(),
            recovered,
        },
    );
    (id, result)
}

/// #1710: the recovering parse of `path`'s `content`, when its strict parse
/// fails in the parser — `None` for clean source, and for source that doesn't
/// lex. Cached with the strict parse (one entry, one content check), so a file
/// that stays broken keeps the same `ExprId`s across calls, and the strict
/// result is still the `Err` the build reads ([DECISION E]).
pub fn cached_recovery(path: &Path, content: &str) -> Option<Arc<parser::Recovered>> {
    let mut state = CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (id, _) = cached_parse_in(&mut state, path, content, PARSE_CACHE_CAP);
    state.entries.get(&id).and_then(|e| e.recovered.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each test uses its own unique path (a fresh `FileId`, never reused by
    /// another test) — `CACHE` is a process-global `static`, so tests running
    /// in the same binary share it; colliding on a real path would make one
    /// test's cache entry leak into another's assertions.
    fn unique_path(name: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        PathBuf::from(format!("/parse-cache-test/{n}-{name}.bynk"))
    }

    const CLEAN: &str = "commons demo\n\ntype T = { n: Int }\n";
    const BROKEN: &str = "commons demo\n\ntype T = {\n";

    #[test]
    fn file_id_is_stable_across_calls_for_the_same_path() {
        let p = unique_path("stable");
        let a = file_id_for(&p);
        let b = file_id_for(&p);
        assert_eq!(a, b);
    }

    #[test]
    fn file_id_is_stable_even_after_the_files_content_changes() {
        let p = unique_path("edited");
        let (id_before, _) = cached_parse(&p, CLEAN);
        let (id_after, _) = cached_parse(&p, "commons demo\n\ntype T = { n: String }\n");
        assert_eq!(
            id_before, id_after,
            "a FileId is a path identity, not a content one"
        );
    }

    #[test]
    fn distinct_paths_get_distinct_file_ids() {
        let a = file_id_for(&unique_path("a"));
        let b = file_id_for(&unique_path("b"));
        assert_ne!(a, b);
    }

    /// The R2.2 collision #1540 was about: `next_file_id` starts at `0`
    /// (`ParseCacheState`'s `Default`), so the very first file this counter
    /// ever interns gets `FileId(0)` — indistinguishable from
    /// `FileId::default()` unless `FileId`'s own `Default` is `UNKNOWN`. A
    /// fresh, locally-owned state (not the process-global `CACHE`, which
    /// other tests may have already advanced) is what makes "first file
    /// interned" reproducible here.
    #[test]
    fn the_first_file_a_fresh_counter_interns_is_not_the_default_file_id() {
        let mut state = ParseCacheState::default();
        let id = file_id_for_locked(&mut state, &unique_path("first"));
        assert_eq!(id, FileId(0));
        assert_ne!(id, FileId::default());
    }

    #[test]
    fn cached_parse_returns_the_same_units_on_a_repeat_call_with_identical_content() {
        let p = unique_path("repeat");
        let (id1, r1) = cached_parse(&p, CLEAN);
        let (id2, r2) = cached_parse(&p, CLEAN);
        assert_eq!(id1, id2);
        let (u1, _) = r1.expect("clean source parses");
        let (u2, _) = r2.expect("clean source parses");
        assert!(
            Arc::ptr_eq(&u1, &u2),
            "a repeat call must be served from cache, not re-parsed"
        );
    }

    #[test]
    fn cached_parse_reparses_when_content_changes() {
        let p = unique_path("dirty");
        let (_, r1) = cached_parse(&p, CLEAN);
        let (_, r2) = cached_parse(&p, "commons demo\n\ntype T = { n: String }\n");
        let (u1, _) = r1.expect("clean source parses");
        let (u2, _) = r2.expect("clean source parses");
        assert!(
            !Arc::ptr_eq(&u1, &u2),
            "a content change must trigger a fresh parse"
        );
    }

    /// #1710: broken syntax also carries its recovering parse, cached in the
    /// same entry, so a repeat call returns the very same recovery (the same
    /// `ExprId`s, stable across project analyses) and the strict result stays
    /// the `Err`. Clean source has none.
    #[test]
    fn cached_recovery_is_cached_beside_the_strict_error() {
        let p = unique_path("recovery");
        let r1 = cached_recovery(&p, BROKEN).expect("broken source recovers");
        let r2 = cached_recovery(&p, BROKEN).expect("broken source recovers");
        assert!(
            Arc::ptr_eq(&r1, &r2),
            "the recovery must be cached, not re-parsed"
        );
        assert!(
            cached_parse(&p, BROKEN).1.is_err(),
            "the strict result stays the error"
        );

        let clean = unique_path("recovery-clean");
        let (_, strict) = cached_parse(&clean, "commons demo\n\nfn one() -> Int { 1 }\n");
        assert!(strict.is_ok());
        assert!(cached_recovery(&clean, "commons demo\n\nfn one() -> Int { 1 }\n").is_none());
    }

    /// #2807: source that doesn't lex recovers too: the strict result carries
    /// the lexer's errors, and the recovering parse covers the rest of the file.
    #[test]
    fn source_that_does_not_lex_still_recovers() {
        let p = unique_path("unlexable");
        let src = "commons demo\n\nfn s() -> String { \"unterminated }\n\nfn t() -> Int { 1 }\n";
        assert!(cached_parse(&p, src).1.is_err());
        let rec = cached_recovery(&p, src).expect("a lex error recovers");
        assert_eq!(rec.units.len(), 1);
    }

    /// [DECISION E]: broken syntax is cached too — as an `Err`, not silently
    /// dropped — so a caller that needs the real errors (the build path) gets
    /// them from the cache exactly like a clean parse.
    #[test]
    fn cached_parse_caches_the_error_for_broken_syntax() {
        let p = unique_path("broken");
        let (_, r1) = cached_parse(&p, BROKEN);
        let (_, r2) = cached_parse(&p, BROKEN);
        assert!(r1.is_err());
        let e1 = r1.unwrap_err();
        let e2 = r2.unwrap_err();
        assert!(
            Arc::ptr_eq(&e1, &e2),
            "a repeat call on the same broken content must be cached too"
        );
    }

    /// [DECISION D]'s own proof: two files, parsed in two separate
    /// `cached_parse` calls (never a single `phase_parse`-style batch), must
    /// still receive disjoint `ExprId` ranges — the hazard this decision
    /// closes is specifically the *cross-call* case a single shared counter
    /// within one call never had to worry about.
    #[test]
    fn expr_ids_never_collide_across_separate_calls() {
        let p1 = unique_path("exprs-a");
        let p2 = unique_path("exprs-b");
        // Parse two identical, nontrivial files (each with a function body
        // tail expression, so each consumes at least one real ExprId) in two
        // separate `cached_parse` calls — never batched through one shared
        // counter the way a single `phase_parse` call already guarantees.
        let src = "commons demo\n\nfn f(x: Int) -> Int {\n  x + 1\n}\n";
        let (_, r1) = cached_parse(&p1, src);
        let (_, r2) = cached_parse(&p2, src);
        let (u1, _) = r1.expect("parses");
        let (u2, _) = r2.expect("parses");

        fn fn_tail_expr_id(u: &SourceUnit) -> bynk_syntax::ast::ExprId {
            let SourceUnit::Commons(c) = u else {
                panic!("expected commons")
            };
            let bynk_syntax::ast::CommonsItem::Fn(f) = &c.items[0] else {
                panic!("expected fn")
            };
            f.body.tail.id
        }
        let id1 = fn_tail_expr_id(&u1[0]);
        let id2 = fn_tail_expr_id(&u2[0]);
        assert_ne!(
            id1, id2,
            "two files parsed in separate cached_parse calls must never share an ExprId"
        );
    }

    // -- Staleness fixture (issue #1515's own "Done when": rename, deletion,
    //    concurrent-edit interleaving — not just a byte-golden pass). --

    /// A rename is, to this cache, an old path that stops being queried and a
    /// new path that starts being — there is no "move" operation to get
    /// wrong, but the new path must get its own independent `FileId` and
    /// parse result, uncontaminated by whatever the old path held.
    #[test]
    fn a_renamed_file_gets_its_own_independent_entry() {
        let old_path = unique_path("renamed-old");
        let new_path = unique_path("renamed-new");
        let (old_id, old_result) = cached_parse(&old_path, CLEAN);
        // The "rename": the same content now arrives under a new path, as if
        // the old path had moved. Nothing ever queries `old_path` again.
        let (new_id, new_result) = cached_parse(&new_path, CLEAN);

        assert_ne!(old_id, new_id, "a renamed file is a new path identity");
        let (old_units, _) = old_result.expect("clean source parses");
        let (new_units, _) = new_result.expect("clean source parses");
        assert!(
            !Arc::ptr_eq(&old_units, &new_units),
            "the new path must not silently reuse the old path's cache entry"
        );
        // The old path's entry is still independently queryable and correct
        // — a rename doesn't corrupt what's left behind, it just stops being
        // read.
        let (old_id_again, _) = cached_parse(&old_path, CLEAN);
        assert_eq!(old_id, old_id_again);
    }

    /// A deleted file simply stops being queried — nothing in this cache's
    /// own API models deletion explicitly (discovery, one layer up, just
    /// stops listing the path). The property worth proving is that an
    /// orphaned entry cannot corrupt a *different* path's own result, and
    /// that the cap-driven eviction `cached_parse` already performs (mirrors
    /// `PROJECT_UNIT_CACHE_CAP`'s own precedent) recovers cleanly — a path
    /// evicted and later re-queried reparses correctly rather than serving
    /// stale or corrupted state.
    ///
    /// Drives a small, locally-owned `ParseCacheState` through
    /// `cached_parse_in` directly (PR #1520's own bot review, finding #2)
    /// rather than forcing the real, global `CACHE` past its real,
    /// 4096-entry cap — that would race every other test in this module
    /// asserting on the same process-wide `static` (`cargo test` runs one
    /// binary's tests concurrently by default) and cost thousands of real
    /// parses to do it. A local state with a cap of 2 exercises the exact
    /// same eviction branch precisely, cheaply, and in isolation.
    #[test]
    fn an_evicted_entry_reparses_correctly_rather_than_serving_stale_state() {
        let mut state = ParseCacheState::default();
        let cap = 2;
        let survivor = PathBuf::from("/parse-cache-test/evict-survivor.bynk");
        let (_, before) = cached_parse_in(&mut state, &survivor, CLEAN, cap);
        let (survivor_units_before, _) = before.expect("clean source parses");

        // Push past the cap with filler paths — the last one triggers the
        // "past the cap, clear and let entries repopulate lazily" eviction,
        // clearing `survivor`'s own entry along with everything else.
        for i in 0..cap {
            let p = PathBuf::from(format!("/parse-cache-test/evict-filler-{i}.bynk"));
            let _ = cached_parse_in(&mut state, &p, CLEAN, cap);
        }

        let (_, after) = cached_parse_in(&mut state, &survivor, CLEAN, cap);
        let (survivor_units_after, _) = after.expect("clean source parses");
        // Correctness, not identity: post-eviction the entry is necessarily
        // re-parsed (a fresh `Arc`), but it must still be the *same* parse of
        // the *same* content — not stale, not corrupted by whichever filler
        // entry happened to occupy the cap-cleared map.
        assert_eq!(
            format!("{survivor_units_before:?}"),
            format!("{survivor_units_after:?}"),
            "a re-parsed-after-eviction file must produce the same result as before eviction"
        );
    }

    /// Concurrent edits: many threads racing `cached_parse` calls against the
    /// *same* path with *different* content must never panic or corrupt the
    /// cache (`PROJECT_UNIT_CACHE`'s own `Mutex`-protected precedent, kept
    /// here) — and once every writer has finished, the cache must correctly
    /// reflect whichever content a query actually asks for next, not some
    /// torn mix of two threads' writes.
    #[test]
    fn concurrent_edits_to_the_same_path_never_corrupt_the_cache() {
        let path = Arc::new(unique_path("concurrent"));
        let variants: Vec<String> = (0..8)
            .map(|i| format!("commons demo\n\ntype T{i} = {{ n: Int }}\n"))
            .collect();

        std::thread::scope(|scope| {
            for v in &variants {
                let path = Arc::clone(&path);
                scope.spawn(move || {
                    for _ in 0..20 {
                        let (_, result) = cached_parse(&path, v);
                        assert!(result.is_ok(), "every variant here is syntactically valid");
                    }
                });
            }
        });

        // After the race, a fresh query with a known, distinct piece of
        // content must reparse and return exactly that content's own result
        // — proving the cache settled into a consistent state, not a torn one
        // that panics or returns nonsense on the next access.
        let (_, tail) = cached_parse(&path, CLEAN);
        let (units, _) = tail.expect("clean source parses");
        assert_eq!(units.len(), 1);
    }
}
