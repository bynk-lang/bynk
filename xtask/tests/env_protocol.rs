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
//! - **Every entry is still read.** Each reader an entry lists still spells, in
//!   code rather than a comment, the entry's name, or each fixed member of a
//!   prefix family that readers look up by name (the `BYNK_DEV_ACK_` five).
//!
//! The directories scanned are `env_protocol::ReaderDir::ALL`, so a reader that
//! moves changes one line there. The test runtime's readers are the `testing/` sources under
//! `bynk-emit/runtime/src` (1B.3a, #4047), not the spliced artifacts.

use bynk_project::env_protocol::{self, Entry, PROTOCOL, ReaderDir, Shape};
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

/// Fixed members of a prefix family that no reader spells, because the reader
/// derives them from the prefix at run time.
const DERIVED_MEMBERS: &[&str] = &[env_protocol::CALL_FROM_BYNK_DEV];

/// The names a reader of `entry` must spell: each fixed member a reader looks
/// up by name (the five `BYNK_DEV_ACK_` acknowledgements), else the entry's own
/// name.
fn required(entry: &Entry) -> Vec<&'static str> {
    let members: Vec<&'static str> = match entry.shape {
        Shape::Exact => Vec::new(),
        Shape::Prefix { members } => members
            .iter()
            .copied()
            .filter(|m| !DERIVED_MEMBERS.contains(m))
            .collect(),
    };
    if members.is_empty() {
        vec![entry.name]
    } else {
        members
    }
}

/// The names `entry` requires ([`required`]) that `src` does not spell as a
/// whole token in code: `BYNK_DEV` is not read by a file that spells only
/// `BYNK_DEV_ACK_AI`, and a name in a `//` or `*` comment line is not a read.
fn unread(entry: &Entry, src: &str) -> Vec<&'static str> {
    let code: String = src
        .lines()
        .filter(|line| {
            let t = line.trim_start();
            !(t.starts_with("//") || t.starts_with('*') || t.starts_with("/*"))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let spelt: BTreeSet<&str> = tokens(&code).into_iter().collect();
    required(entry)
        .into_iter()
        .filter(|name| !spelt.contains(name))
        .collect()
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
            for name in unread(entry, &src) {
                problems.push(format!("{path} no longer reads `{name}`"));
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
    assert_eq!(
        unread(seed, "process.env.BYNK_TEST_SEDE"),
        [env_protocol::TEST_SEED]
    );
    assert!(unread(seed, "process.env.BYNK_TEST_SEED").is_empty());
    // A longer name is not the exact one, and a comment is not a read.
    let dev = env_protocol::entry_of(env_protocol::DEV).unwrap();
    assert_eq!(unread(dev, "env.BYNK_DEV_ACK_AI"), [env_protocol::DEV]);
    assert_eq!(
        unread(dev, "// sets `BYNK_DEV` on every Worker\n * BYNK_DEV"),
        [env_protocol::DEV]
    );
    assert!(unread(dev, "if (vars?.BYNK_DEV !== \"1\") return;").is_empty());
}

#[test]
fn a_family_is_read_only_if_each_member_looked_up_by_name_is() {
    let acks = env_protocol::entry_of(env_protocol::DEV_ACK_AI).unwrap();
    let all = "\"BYNK_DEV_ACK_AI\" \"BYNK_DEV_ACK_VECTORIZE\" \"BYNK_DEV_ACK_AI_SEARCH\" \
               \"BYNK_DEV_ACK_MTLS\" \"BYNK_DEV_ACK_ANALYTICS_SQL\"";
    assert!(unread(acks, all).is_empty());
    // The prefix alone, in a comment, does not read the family.
    assert_eq!(
        unread(acks, "// `BYNK_DEV_ACK_<KIND>`").len(),
        5,
        "every member is unread"
    );
    let without_mtls = all.replace("\"BYNK_DEV_ACK_MTLS\"", "");
    assert_eq!(unread(acks, &without_mtls), [env_protocol::DEV_ACK_MTLS]);
    // A member derived at run time is not required: the prefix is.
    let from = env_protocol::entry_of(env_protocol::CALL_FROM_PREFIX).unwrap();
    assert!(unread(from, "export const EDGE_FROM_PREFIX = \"BYNK_CALL_FROM_\";").is_empty());
    assert_eq!(unread(from, ""), [env_protocol::CALL_FROM_PREFIX]);
}
