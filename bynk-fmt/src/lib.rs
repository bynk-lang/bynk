//! `bynk-fmt` — the Bynk formatter.
//!
//! A real leaf crate: the formatter is an AST-walk over the `bynk-syntax`
//! types, so it depends on `bynk-syntax` only and never links the compiler
//! (resolver/checker/emitter). Slice 2 of the crate-decomposition track moved
//! the implementation down here from `bynkc::fmt` and re-pointed it onto the
//! `bynk-syntax` leaf, turning the former cosmetic façade into the home of the
//! formatter itself. `bynkc`'s own `bynkc fmt` command reaches it via
//! `bynk-driver::run_fmt` (R10.4 residue, #1048: `bynkc` no longer re-exports
//! this crate as `bynkc::fmt`).

mod config;
mod fmt;

pub use fmt::{FormatError, FormatOptions, IndentStyle, format_source, normalize_line_endings};
// v0.123 (editor-currency slice 2): the surface renderers for an expression and
// a refinement, exposed so `bynk-lsp` hover renders predicates / `where`
// clauses through the formatter's own logic rather than a copy that could drift.
pub use fmt::{expr_to_string, refinement_to_string};
// v0.137.0 (ADR 0161): the storage-annotation renderer, exposed so `bynk-lsp`
// agent-state hover renders a `store` field's `@indexed`/`@bounded`/… through
// the formatter's own logic rather than a drift-prone copy.
pub use fmt::annotation_to_string;
// v0.166 (#616): the string-literal escaper, exposed for the same reason — an
// actor's `auth = Scheme(secret = "…")` config holds the *unescaped* value (the
// parser resolves escapes at lex time), so `bynk-lsp` hover must re-escape it to
// render valid Bynk, exactly as `format_actor` does. Without it the two renderers
// agree until the value contains a `"` or a `\`.
pub use fmt::escape_string;

// #972: the `[fmt]` section of a project's `bynk.toml`, layered under the CLI's
// own flags by `bynkc fmt`/`bynk fmt` and read directly by `bynk-lsp` for
// format-on-save. One reader for one section — the two front-ends previously
// had only the language server reading it at all, so a project that set a style
// there saw the editor and the command line disagree.
#[cfg(feature = "schema")]
pub use config::fmt_table_schema;
pub use config::{ConfigError, FmtConfig, MANIFEST, find_manifest};
