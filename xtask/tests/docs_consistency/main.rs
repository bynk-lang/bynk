//! The docs and index consistency checks: generated reference pages, indexes
//! and diagrams against the files they describe (C4b, #2940).
//!
//! None of these is a compiler test. They read the repository's Markdown, JSON
//! and manifests, and need no compiled test corpus, so they live in `xtask` (whose
//! `drift` CI job runs them when only a drift input changed) instead of the
//! integration corpus. One binary, not eleven: linking dominates a test crate's
//! cost (see `bynk-tests/tests/it.rs`).
//!
//! A test's name gains its file's stem as a prefix. Run one file's with
//! `cargo test -p xtask --test docs_consistency decisions_index::`.

mod cli_reference;
mod decisions_index;
mod diagnostics_registry;
mod doc_version;
mod generated_grammar_json;
mod generated_site_json;
mod grammar_coverage;
mod grammar_reference;
mod keywords_reference;
mod readme_layering;
mod textmate_keywords;
