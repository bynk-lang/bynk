// The `bynkc` binary the corpus spawns. See `[[bin]]` in `bynk-tests/Cargo.toml`.
//
// `bynkc`'s `main.rs` uses these two crates; naming them here keeps the
// dependency edges visible to the `unused_deps` probe, which reads this file
// and cannot see through the `include!`.
use bynk_driver as _;
use clap as _;

include!("../../../bynkc/src/main.rs");
