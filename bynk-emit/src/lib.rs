//! Bynk's TypeScript emission, plus the per-unit build sequencing that drives
//! it — the layer above `bynk-project` (discovery, the dependency graph),
//! `bynk-check` (all semantic checking, R3.5), and (since the P7.12 crate
//! carve) `bynk-ir`/`bynk-lower` (the typed IR and its `&CheckedProgram → Ir`
//! lowering pass).
//!
//! `project` owns `compile_project`/`run_checks`: the two-pass sequence over
//! a project's units — discover and parse (`bynk-project`), then resolve,
//! type-check (`bynk-check`) and emit each unit with full visibility of what
//! it `uses`/`consumes`. `emitter` lowers a checked program to TypeScript.
//! Input: a project tree (or an in-memory overlay). Output: TypeScript files
//! plus diagnostics — this crate originates none of its own (P5.5,
//! `design/tracks/semantics-in-the-checker.md` §3.5, R10.1).
//!
//! Extracted from `bynkc` as slice 4 of the crate-decomposition track over
//! `bynk-syntax` + `bynk-check`. Behaviour is unchanged; `bynkc` depends on this
//! crate and re-exports its modules so its public API (`compile_project`,
//! `ProjectOutput`, …) and the binary are untouched.

pub mod emitter;
pub mod project;

#[cfg(test)]
pub(crate) mod testkit;

use bynk_check::{checker, resolver};
use bynk_syntax::{CompileError, lexer, parser};

/// A single-file compile that also returns the non-failing warnings produced on
/// success — what a CLI prints (v0.89, ADR 0117). [`compile`] is the
/// warning-discarding convenience over this.
///
/// Lives in `bynk-emit` (slice 7 precedent, alongside [`NODE_MAJOR_FLOOR`]) so
/// both `bynkc` and the `bynk` driver can compile a self-contained single-file
/// commons in-process without depending on each other; `bynkc` re-exports it so
/// `bynkc::compile_with_warnings` and `bynkc::Compiled` are unchanged.
pub struct Compiled {
    pub ts: String,
    pub warnings: Vec<CompileError>,
}

/// Compile a single Bynk source string to a TypeScript string.
///
/// Parses the input as a self-contained, single-file commons with no `uses`
/// against other commons. Use [`project::compile_project`] for multi-file
/// projects or for any source that declares `uses`. `filename` is used only for
/// diagnostic rendering.
pub fn compile(source: &str, filename: &str) -> Result<String, Vec<CompileError>> {
    compile_with_warnings(source, filename).map(|c| c.ts)
}

/// The warning-preserving single-file compile behind [`compile`]. See [`Compiled`].
pub fn compile_with_warnings(source: &str, _filename: &str) -> Result<Compiled, Vec<CompileError>> {
    let (program, warnings) = check_single_file(source, SingleFileMode::Build)?;
    Ok(Compiled {
        ts: emitter::emit(&program),
        warnings,
    })
}

/// Check a single-file commons without building it: the same diagnostics as
/// [`compile_with_warnings`], minus the build-only ones, and no emission.
/// Returns the non-failing warnings on success. The single-file half of
/// `bynk check` / `bynkc check` (`bynk_driver::run_check`); a directory goes
/// through the project pipeline in `Mode::Check` instead.
///
/// #1883, slice 1a (review of #1903): before this, `run_check` reused
/// [`compile_with_warnings`], so `bynk check page.bynk` failed with the
/// build-only asset refusal, contradicting §5.12's "checking is unaffected".
pub fn check_with_warnings(
    source: &str,
    _filename: &str,
) -> Result<Vec<CompileError>, Vec<CompileError>> {
    check_single_file(source, SingleFileMode::Check).map(|(_, warnings)| warnings)
}

/// Whether a single-file run will go on to emit (`Build`) or only report
/// (`Check`). Only build-only diagnostics differ: today, the asset gate.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SingleFileMode {
    Check,
    Build,
}

/// The shared single-file pipeline: lex, parse, check, certify.
fn check_single_file(
    source: &str,
    mode: SingleFileMode,
) -> Result<(checker::CheckedProgram, Vec<CompileError>), Vec<CompileError>> {
    let tokens = lexer::tokenize(source).map_err(|e| vec![e])?;
    // ADR 0117: parse-time warnings (orphan doc blocks) ride alongside the
    // AST — they surface with the build's warnings instead of failing it.
    let (commons, mut warnings) = match parser::parse_with_warnings(&tokens, source) {
        Ok(parsed) => parsed,
        // #1663: the strict parse stops at the first syntax error. Diagnose the
        // file as the editor does instead, and still refuse to compile it.
        Err(strict) => {
            return Err(bynk_check::recovery::diagnose_unparsable(
                &tokens, source, strict,
            ));
        }
    };
    // v0.20a: function types are confined to non-boundary positions — the same
    // rule the project path applies.
    let mut boundary_errors = Vec::new();
    let boundary_types = bynk_check::project_model::collect_type_decls(commons.items.iter());
    bynk_check::project_model::check_function_type_boundary_items(
        &commons.items,
        &boundary_types,
        &mut boundary_errors,
    );
    // #1883: the asset declaration rules (path, duplicates, name conflicts),
    // which the project path raises in `build_unit_table`; and, in a build
    // only, `bynk.asset.needs_project` — a single file has no project root to
    // find `assets/` under and emits no Worker to serve it beside.
    boundary_errors.extend(bynk_check::assets::single_file_asset_errors(
        &commons.items,
        mode == SingleFileMode::Build,
    ));
    // #2387: the same for `embed` — its path and name rules, and, in
    // a build only, `bynk.embed.needs_project` — a single file has no project root
    // to find `embeds/` under.
    boundary_errors.extend(bynk_check::embeds::single_file_embed_errors(
        &commons.items,
        mode == SingleFileMode::Build,
    ));
    // #2617: the `const` name rules (duplicates, name conflicts), which the
    // project path raises in `build_unit_table`.
    boundary_errors.extend(bynk_check::consts::single_file_const_errors(&commons.items));
    // #1663: a boundary or resolve error is local to its declaration, so the
    // checker still runs over every declaration and its diagnostics join
    // theirs (minus echoes); any error still refuses the compile.
    let item_spans: Vec<_> = commons.items.iter().map(|i| i.span()).collect();
    let (resolved, resolve_errors) = resolver::resolve_recovering(commons);
    let mut errors = boundary_errors;
    errors.extend(resolve_errors.iter().cloned());
    let typed = match checker::check(resolved) {
        Ok(typed) if errors.is_empty() => typed,
        Ok(_) => return Err(errors),
        Err(checked) => {
            errors.extend(resolver::without_resolve_echoes(
                checked,
                &resolve_errors,
                &item_spans,
            ));
            return Err(errors);
        }
    };
    warnings.extend(typed.warnings.clone());
    // T3.7 (R3.10): `check` already gated on error-severity diagnostics, so
    // `typed.warnings` — the only diagnostics left riding along with it — can
    // never contain one; `certify` re-asserts that structurally rather than
    // trusting the caller not to skip it.
    let program = checker::certify(typed, warnings.clone()).unwrap_or_else(|_| {
        panic!("bynk internal error: check() already gated on error-severity diagnostics")
    });
    Ok((program, warnings))
}

/// Minimum supported Node.js **major** version for the `node` platform binding
/// and for running Bynk's emitted TypeScript.
///
/// Single source of truth for the Node floor: the emitted code targets it, the
/// `bynk` driver's `doctor` command compares a detected `node` against it, and
/// `bynkc`'s CLI re-exports it rather than restating the number. Lives in
/// `bynk-emit` (which emits the TS that runs on Node) so both binaries share one
/// definition (slice 7; was a `bynkc` const before the driver dropped that dep).
///
/// #1674: 22, the oldest Node major still in support (18 reached end-of-life in
/// April 2025, 20 in April 2026), and the one `bynkc test --inspect` needs. So
/// `bynk doctor`'s `ok` means the commands work, not merely that Node exists.
/// The `--inspect` path checks its own minor floor (22.6 to run, 22.18 for
/// source-mapped breakpoints) and says so when it isn't met.
pub const NODE_MAJOR_FLOOR: u32 = 22;

/// The minor version within [`NODE_MAJOR_FLOOR`] the floor sits at: 22.13,
/// the first 22.x whose built-in `node:sqlite` loads without
/// `--experimental-sqlite` (standard-library P4a, #2602: the `D1` test double
/// runs on it). CI's floor job installs exactly this version.
pub const NODE_MINOR_FLOOR: u32 = 13;

/// The Node floor as `(major, minor)`: what `bynk doctor` compares against.
pub const NODE_FLOOR: (u32, u32) = (NODE_MAJOR_FLOOR, NODE_MINOR_FLOOR);

/// #1672: the oldest TypeScript major the emitted output is verified against.
/// CI type-checks every positive fixture under it (`tsc_verify`, with
/// `typescript@5` installed on every test leg).
pub const TYPESCRIPT_MAJOR_FLOOR: u32 = 5;

/// #1672: the newest TypeScript major the emitted output is verified against —
/// the one users get from `npm install -g typescript`. CI type-checks every
/// positive fixture under it too (a second `tsc_verify` pass on the Linux leg),
/// the `npx` fallbacks provision it, and `bynk doctor` reports a `tsc` newer
/// than it as untested.
pub const TYPESCRIPT_MAJOR_TESTED: u32 = 7;

/// I2 (#1971): the value of every edge key off the deployed path. `bynk dev`
/// passes it for each edge a Worker is on (`BYNK_CALL_TO_<callee>`,
/// `BYNK_CALL_FROM_<caller>`, named by `bynk_check::wire_format`), and the
/// generated test harnesses put it in every participant's `env`. Fixed rather
/// than random so two `bynk dev --context` sessions run side by side still
/// admit each other's calls. It guards nothing, so it must never reach a
/// deployed Worker: `bynk deploy` mints one key per edge, and none is ever
/// written into a generated `wrangler.toml`.
pub const LOCAL_CALL_KEY: &str = "bynk-local-call-key";

/// State-migrations S4 (#2259): the call signature and the sweep route's
/// names, for `bynk migrate sweep`, which signs as the runtime verifies.
pub use bynk_check::wire_format;
/// I2 (#1971): the edge keys' names, for the driver (`bynk dev`, `bynk
/// deploy`), which sees the compiler only through this crate.
pub use bynk_check::wire_format::{EDGE_FROM_PREFIX, EDGE_TO_PREFIX, edge_key_from, edge_key_to};

pub use emitter::wrangler::{
    COMPATIBILITY_DATE, DEFAULT_COMPATIBILITY_FLAGS, WRANGLER_MIN, compatibility_flags,
};

// `write_output`/`write_document` moved to `bynk-driver` (#1047, R2.3/
// T0.7 residue): every caller was already at driver level, so this crate
// never needed direct filesystem access for it — the pure move closes it
// out of this crate's `fs_below_driver` count. See `bynk-driver::output`.
