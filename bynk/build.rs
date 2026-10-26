//! #2463: embed the Bynk Book in the driver, so `bynk book` answers from the
//! text that matches this exact toolchain — offline, with no file to install
//! beside the binary (the extension downloads `bynk` as a single file).
//!
//! The bundle is `site/public/llms-full.txt`, which `site/scripts/build-llms-full.mjs`
//! generates (and the site gitignores). A release build generates it first; a
//! build without it — a plain `cargo build`, or `cargo install` from a crate
//! tarball that has no `site/` — embeds an empty bundle, which `bynk book`
//! reports so callers can fall back to the hosted Book.

use std::{env, fs, path::PathBuf};

fn main() {
    let source = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"))
        .join("../site/public/llms-full.txt");
    println!("cargo:rerun-if-changed={}", source.display());
    let bundle = fs::read_to_string(&source).unwrap_or_default();
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("book.txt");
    fs::write(out, bundle).expect("write the embedded Book bundle");
}
