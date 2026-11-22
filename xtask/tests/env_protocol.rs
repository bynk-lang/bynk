//! Back-half paydown 1D.5a (#4003, Decision C): the hand-written TypeScript
//! readers of the `BYNK_*` protocol, held to the Rust table
//! `bynk_project::env_protocol::PROTOCOL`.
//!
//! The readers keep their string literals, so emitted output does not change.
//! This check makes a TypeScript misspelling fail `cargo test -p xtask`, as a
//! Rust one fails to compile:
//!
//! - **Every token is accounted for.** Each `BYNK_` or `__BYNK_` token in a
//!   reader directory is a table name (an entry, or a prefix family's fixed
//!   member) or one of the runtime's own constants in [`RUNTIME_CONSTANTS`].
//! - **Every entry is still read.** Each reader an entry lists still spells the
//!   entry's name.
//!
//! The directories scanned are `env_protocol::ReaderDir::ALL`, so a reader that
//! moves (1B.3 moves the test runtime into `bynk-emit`) changes one line there.

use bynk_project::env_protocol::{self, Entry, PROTOCOL, ReaderDir};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// `__BYNK_` names the runtime defines for itself, which are not part of the
/// protocol: module constants, not names Rust sets.
const RUNTIME_CONSTANTS: &[&str] = &[
    "__BYNK_CLOCK_SKEW_S",
    "__BYNK_DECIMAL_MAX_SCALE",
    "__BYNK_DECIMAL_MAX_TEXT",
    "__BYNK_JWKS_REFETCH_COOLDOWN_MS",
    "__BYNK_JWKS_TTL_MS",
    "__BYNK_TIMED_OUT",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits in the repo root")
        .to_path_buf()
}

fn token_pattern() -> regex::Regex {
    regex::Regex::new(r"\b(__)?BYNK_[A-Z0-9_]+").expect("the token pattern compiles")
}

/// Every `BYNK_` token in `src`, in order.
fn tokens(src: &str) -> Vec<&str> {
    token_pattern().find_iter(src).map(|m| m.as_str()).collect()
}

/// The tokens in `src` that are neither a protocol name nor a runtime constant.
fn unaccounted(src: &str) -> Vec<String> {
    let known: BTreeSet<&str> = env_protocol::all_names()
        .chain(RUNTIME_CONSTANTS.iter().copied())
        .collect();
    tokens(src)
        .into_iter()
        .filter(|t| !known.contains(t))
        .map(str::to_string)
        .collect()
}

/// Whether `src` spells `entry`'s name as a whole token (`BYNK_DEV` is not read
/// by a file that spells only `BYNK_DEV_ACK_AI`). A prefix family's fixed
/// members need not be spelt: `BYNK_CALL_FROM_BYNK__DEV` is derived from the
/// prefix at run time.
fn reads(entry: &Entry, src: &str) -> bool {
    tokens(src).contains(&entry.name)
}

fn ts_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            ts_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "ts") {
            out.push(path);
        }
    }
}

#[test]
fn every_bynk_token_in_a_reader_is_a_protocol_name() {
    let root = repo_root();
    let mut files = Vec::new();
    for dir in ReaderDir::ALL {
        let before = files.len();
        ts_files(&root.join(dir.path()), &mut files);
        assert!(files.len() > before, "no TypeScript under {}", dir.path());
    }
    let mut problems = Vec::new();
    let mut seen_constants = BTreeSet::new();
    for file in &files {
        let src = std::fs::read_to_string(file).unwrap();
        for token in tokens(&src) {
            if RUNTIME_CONSTANTS.contains(&token) {
                seen_constants.insert(token.to_string());
            }
        }
        for token in unaccounted(&src) {
            problems.push(format!(
                "{}: `{token}` is not in bynk_project::env_protocol::PROTOCOL",
                file.strip_prefix(&root).unwrap_or(file).display()
            ));
        }
    }
    for constant in RUNTIME_CONSTANTS {
        if !seen_constants.contains(*constant) {
            problems.push(format!(
                "`{constant}` is allowed as a runtime constant but no reader spells it: \
                 delete it from RUNTIME_CONSTANTS"
            ));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn every_entry_is_still_read_where_the_table_says() {
    let root = repo_root();
    let mut problems = Vec::new();
    for entry in PROTOCOL {
        for reader in entry.readers {
            let path = reader.path();
            let Ok(src) = std::fs::read_to_string(root.join(&path)) else {
                problems.push(format!("{}: reader {path} does not exist", entry.name));
                continue;
            };
            if !reads(entry, &src) {
                problems.push(format!("{path} no longer reads `{}`", entry.name));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn a_misspelt_name_is_refused() {
    assert_eq!(
        unaccounted("const seed = process.env.BYNK_TEST_SEDE;"),
        ["BYNK_TEST_SEDE"]
    );
    // A concrete member of a family is not a name a reader may spell.
    assert_eq!(
        unaccounted("env.BYNK_DATASET_ANALYTICS_SIGNUPS"),
        ["BYNK_DATASET_ANALYTICS_SIGNUPS"]
    );
    assert!(unaccounted("process.env.BYNK_TEST_SEED; `BYNK_DATASET_${b}`").is_empty());
    assert!(unaccounted("const __BYNK_TIMED_OUT = Symbol();").is_empty());
}

#[test]
fn a_reader_that_stops_reading_its_name_is_refused() {
    let seed = env_protocol::entry_of(env_protocol::TEST_SEED).unwrap();
    assert!(!reads(seed, "process.env.BYNK_TEST_SEDE"));
    assert!(reads(seed, "process.env.BYNK_TEST_SEED"));
    // A longer name is not the exact one.
    let dev = env_protocol::entry_of(env_protocol::DEV).unwrap();
    assert!(!reads(dev, "env.BYNK_DEV_ACK_AI"));
    assert!(reads(dev, "env.BYNK_DEV === \"1\""));
}
