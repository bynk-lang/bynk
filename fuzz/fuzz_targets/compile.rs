//! Fuzz the whole single-file pipeline (#516): lex → parse → resolve →
//! check → emit. This is the target that would have caught the
//! mutually-recursive-record stack overflow (#506) — crashes in the middle
//! passes only reproduce on inputs that parse, so the corpus seed
//! (`seed-corpus.sh`) matters more here than for the parse target.
//!
//! Invariant: `bynk_emit::compile` returns `Ok` or `Err` — it never panics and
//! never aborts.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };
    let _ = bynk_emit::compile(source, "fuzz.bynk");
});
