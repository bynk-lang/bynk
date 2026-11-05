//! v0.48: the first-party Bynk sources are now real files embedded via
//! `include_str!` (see `bynk-std/lib/`). These tests give them the
//! standing guarantees they lacked as Rust string literals:
//!
//!  1. **Each source parses** — tokenises + `parse_unit` without errors, so the
//!     stdlib/surface can never ship un-parseable (previously only checked
//!     transitively, when a fixture happened to `uses` it).
//!  2. **Each source is `bynk-fmt`-clean** — formatting is a no-op, so the
//!     first-party sources obey the project's own formatting rules. (Reformatting
//!     a `.bynk` source never changes emitted TypeScript — formatting is
//!     whitespace/trivia only — so this is independent of the byte-identical
//!     emitted-output guarantee, which the golden + tsc_verify suites pin.)
//!
//! Standalone `tsc --strict` over the embedded TypeScript runtime lives in
//! `tsc_verify.rs` (it reuses that file's tsc runner / skip-loudly logic).

use bynk_fmt::{FormatOptions, format_source};
use bynk_syntax::lexer::tokenize;
use bynk_syntax::parser::parse_unit;

/// The first-party Bynk sources, by display name — the single
/// `FIRSTPARTY_SOURCES` list, so this guard covers every first-party commons
/// automatically (`bynk.locale.types` was previously omitted here — #901).
fn sources() -> &'static [(&'static str, &'static str)] {
    bynk_check::firstparty::FIRSTPARTY_SOURCES
}

#[test]
fn every_first_party_source_parses() {
    let mut failures = Vec::new();
    for &(name, src) in sources() {
        let parsed = tokenize(src).and_then(|toks| {
            parse_unit(&toks, src).map_err(|errs| {
                errs.into_iter()
                    .next()
                    .unwrap_or_else(|| panic!("empty error list for {name}"))
            })
        });
        if let Err(e) = parsed {
            failures.push(format!("{name}: {} {}", e.category, e.message));
        }
    }
    assert!(
        failures.is_empty(),
        "first-party sources must tokenise + parse:\n{}",
        failures.join("\n")
    );
}

#[test]
fn every_first_party_source_is_fmt_clean() {
    let opts = FormatOptions::default();
    let mut failures = Vec::new();
    for &(name, src) in sources() {
        match format_source(src, &opts) {
            Ok(formatted) if formatted == src => {}
            Ok(_) => failures.push(format!(
                "{name}: not bynk-fmt-clean (run bynk-fmt over bynk-std/lib/)"
            )),
            Err(e) => failures.push(format!("{name}: format failed ({} errors)", e.errors.len())),
        }
    }
    assert!(
        failures.is_empty(),
        "first-party .bynk sources must be bynk-fmt-clean:\n{}",
        failures.join("\n")
    );
}

/// #901 drift guard: every `pub const *_SRC` in `bynk-std/src/lib.rs` must be a
/// `source:` of an entry in the `UNITS` registry, which `FIRSTPARTY_SOURCES` is
/// a view over (S1, #1945). Otherwise the next first-party commons could be
/// added to `bynk-std/src/lib.rs` yet silently left out of injection, hover,
/// completion and this parse guard — the exact way `bynk.locale` and
/// `bynk.locale.types` were. A pure text scan of the source file, since Rust has
/// no const reflection.
#[test]
fn firstparty_sources_cover_every_src_const() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../bynk-std/src/lib.rs");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

    // The `UNITS: &[Unit] = &[ … ];` registry literal, as text.
    let list_start = text
        .find("pub const UNITS: &[Unit] = &[")
        .expect("UNITS declared in bynk-std/src/lib.rs");
    let list_end = text[list_start..]
        .find("\n];")
        .map(|rel| list_start + rel)
        .expect("the UNITS array is closed with `];` at column 0");
    let list_block = &text[list_start..list_end];

    // Every exported source const: `pub const <NAME>_SRC: &str = include_str!(…)`.
    let mut missing = Vec::new();
    for line in text.lines() {
        let line = line.trim_start();
        let Some(rest) = line.strip_prefix("pub const ") else {
            continue;
        };
        let Some((ident, _)) = rest.split_once(':') else {
            continue;
        };
        let ident = ident.trim();
        // The `.bynk` source consts, not the list itself or binding paths.
        if ident.ends_with("_SRC") && !list_block.contains(&format!("source: {ident},")) {
            missing.push(ident.to_string());
        }
    }

    assert!(
        missing.is_empty(),
        "these `*_SRC` consts are declared in bynk-std/src/lib.rs but missing from \
         the UNITS registry (add an entry, or injection/hover/completion/parse-guard \
         will skip them — #901):\n{}",
        missing.join("\n")
    );
}
