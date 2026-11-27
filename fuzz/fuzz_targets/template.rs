//! Fuzz the typed-HTML template checker (#3927, typed-HTML §11.2): placeholder recognition, the
//! tokeniser and the context model, the compile-time half of every template and fragment.
//!
//! Invariants:
//! - `analyse_template` never panics, and every refusal points inside the input, on a character
//!   boundary.
//! - On every input it accepts, the differential holds: `html5ever`, with scripting on, puts each
//!   placeholder where its context says, and adversarial values stay one whole value each and add
//!   no element or attribute (the same oracle as `bynk-check`'s `proptest` differential).
#![no_main]

#[path = "../../bynk-check/tests/support/html_oracle.rs"]
mod html_oracle;

use bynk_check::html_template::{analyse_template, At};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let a = analyse_template(text);
    for r in &a.analysis.markup {
        if let At::Offset(o) = r.at {
            assert!(o <= text.len() && text.is_char_boundary(o), "refusal at {o} in {} bytes", text.len());
        }
    }
    if let Err(why) = html_oracle::check(text) {
        panic!("{why}");
    }
});
