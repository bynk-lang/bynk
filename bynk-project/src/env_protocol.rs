//! Back-half paydown 1D.5a (#4003): the cross-language `BYNK_*` protocol.
//!
//! The toolchain hands generated TypeScript a small set of names: environment
//! variables on the `node`/`tsx` process that `bynk test` starts, and entries
//! in a Worker's `env` (a `[vars]` entry or `--var`, a binding, a secret) that
//! `bynk dev`, `bynk deploy` and the emitted `wrangler.toml` set. Rust writes
//! each name and TypeScript reads it, so the two spellings must agree.
//!
//! This module is the one definition of each name. Every Rust writer uses
//! these constants (or a `pub use` of one kept at its older path), so a
//! misspelt name in Rust fails to compile. The TypeScript readers are written
//! by hand and keep their string literals, so the emitted output does not
//! change. `xtask/tests/env_protocol.rs` holds them to [`PROTOCOL`] instead:
//! every `BYNK_` token a reader spells must be a name here, and every reader an
//! entry lists must still spell its name.
//!
//! Only the protocol lives here. Names that Rust alone reads or sets
//! (`BYNK_HOME`, `BYNK_NO_INSTALL`, `BYNK_PHASE_TIMING` and the like) stay with
//! their owners, and test gates stay in the tests (#4003, Decision E). No name
//! here may be renamed: each one is part of what a generated program reads.

/// How a name reaches the TypeScript that reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Carrier {
    /// An environment variable on the test process (`process.env`).
    Process,
    /// A Worker variable: a `[vars]` entry or a `wrangler dev --var` (`env`).
    WorkerVar,
    /// A Worker binding's name in the generated `wrangler.toml` (`env`).
    Binding,
    /// A Worker secret (`env`).
    Secret,
}

/// Whether an entry is one name, or the prefix of a family of names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// The name exactly.
    Exact,
    /// A prefix to which a binding, context or kind name is appended. `members`
    /// are the family's fixed names, where it has any.
    Prefix { members: &'static [&'static str] },
}

/// Where the hand-written readers live. [`Reader::path`] joins a reader's file
/// to its directory, so a directory that moves (1B.3 moves the test runtime
/// into `bynk-emit`) changes one line here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReaderDir {
    /// The emitted runtime's sources, `bynk-emit/runtime/src`.
    Runtime,
    /// The test-runtime fragments spliced into generated test mains.
    TestRuntime,
    /// The standard library's binding sources, `bynk-std/lib`.
    Std,
}

impl ReaderDir {
    /// Every reader directory, for a scan that wants them all.
    pub const ALL: [ReaderDir; 3] = [ReaderDir::Runtime, ReaderDir::TestRuntime, ReaderDir::Std];

    /// The directory, from the repository root.
    pub const fn path(self) -> &'static str {
        match self {
            ReaderDir::Runtime => "bynk-emit/runtime/src",
            ReaderDir::TestRuntime => "bynk-driver/src/project/test_runtime",
            ReaderDir::Std => "bynk-std/lib",
        }
    }
}

/// One hand-written TypeScript file that reads a protocol name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reader {
    pub dir: ReaderDir,
    /// The file, relative to [`Reader::dir`].
    pub file: &'static str,
}

impl Reader {
    /// The reader's path from the repository root.
    pub fn path(&self) -> String {
        format!("{}/{}", self.dir.path(), self.file)
    }
}

/// One protocol name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub name: &'static str,
    pub carrier: Carrier,
    pub shape: Shape,
    /// The Rust that sets it, as a path.
    pub set_by: &'static str,
    /// The hand-written TypeScript files that read it. Empty when the only
    /// reader is TypeScript the toolchain emits from this constant.
    pub readers: &'static [Reader],
    /// The Rust that emits a TypeScript reader of it, where one does.
    pub emitted_by: Option<&'static str>,
}

// --- The test process (`bynk test`) ----------------------------------------

/// `ndjson` when the driver wants one JSON event per line from a test run.
pub const TEST_FORMAT: &str = "BYNK_TEST_FORMAT";
/// The JSON test filter `bynk test --filter` hands the generated test main.
pub const TEST_FILTER: &str = "BYNK_TEST_FILTER";
/// The hex seed of a property-test run.
pub const TEST_SEED: &str = "BYNK_TEST_SEED";
/// How long, in milliseconds, a test waits for a deferred effect to settle.
pub const TEST_SETTLE_MS: &str = "BYNK_TEST_SETTLE_MS";
/// The JSON `[test.targets]` a remote test run reaches.
pub const TEST_TARGETS: &str = "BYNK_TEST_TARGETS";
/// The secrets of a remote test run, as JSON.
pub const TEST_SECRETS: &str = "BYNK_TEST_SECRETS";
/// The run id a remote test run tags what it writes with.
pub const TEST_RUN_ID: &str = "BYNK_TEST_RUN_ID";
/// `1` when a smoke run treats an undeclared wire shape as a failure.
pub const TEST_STRICT_WIRE: &str = "BYNK_TEST_STRICT_WIRE";
/// The file a smoke run writes the declared statuses it saw to.
pub const SMOKE_DECLARED_FILE: &str = "BYNK_SMOKE_DECLARED_FILE";
/// `1` when `bynk test --update-snapshots` rewrites snapshots.
pub const UPDATE_SNAPSHOTS: &str = "BYNK_UPDATE_SNAPSHOTS";
/// `1` when the driver wants the snapshot files a run touched reported.
pub const SNAPSHOT_REPORT: &str = "BYNK_SNAPSHOT_REPORT";

// --- A Worker's `env` ------------------------------------------------------

/// `1` on every Worker `bynk dev` serves, so a provider can tell a development
/// session from a deployment.
pub const DEV: &str = "BYNK_DEV";
/// The prefix of the `[dev]` acknowledgement variables
/// (`crate::dev_ack::REMOTE_KINDS`).
pub const DEV_ACK_PREFIX: &str = "BYNK_DEV_ACK_";
/// `[dev] ai = "allow"`.
pub const DEV_ACK_AI: &str = "BYNK_DEV_ACK_AI";
/// `[dev] vectorize = "allow"`.
pub const DEV_ACK_VECTORIZE: &str = "BYNK_DEV_ACK_VECTORIZE";
/// `[dev] ai-search = "allow"`.
pub const DEV_ACK_AI_SEARCH: &str = "BYNK_DEV_ACK_AI_SEARCH";
/// `[dev] mtls = "allow"`.
pub const DEV_ACK_MTLS: &str = "BYNK_DEV_ACK_MTLS";
/// `[dev] analytics-sql = "allow"`.
pub const DEV_ACK_ANALYTICS_SQL: &str = "BYNK_DEV_ACK_ANALYTICS_SQL";
/// The prefix of the `[vars]` entry giving a local Vectorize index its
/// dimensions and metric under `bynk dev`, followed by the binding name.
pub const DEV_INDEX_PREFIX: &str = "BYNK_DEV_INDEX_";
/// The prefix of the `[vars]` entry naming an Analytics Engine dataset's
/// table, followed by the binding name. Reserved in `[test.targets]` vars.
pub const DATASET_PREFIX: &str = "BYNK_DATASET_";
/// The prefix of the secret a callee holds for the edge from one caller,
/// followed by the caller's edge suffix.
pub const CALL_FROM_PREFIX: &str = "BYNK_CALL_FROM_";
/// The prefix of the secret a caller holds for the edge to one callee,
/// followed by the callee's edge suffix.
pub const CALL_TO_PREFIX: &str = "BYNK_CALL_TO_";
/// The edge key from `bynk dev inspect`'s caller, `bynk.dev`: the
/// [`CALL_FROM_PREFIX`] family's one fixed member.
pub const CALL_FROM_BYNK_DEV: &str = "BYNK_CALL_FROM_BYNK__DEV";
/// The `wrangler dev --var` that tells a generated container class its
/// containers are off (`bynk dev` without Docker).
pub const CONTAINERS_UNAVAILABLE: &str = "BYNK_CONTAINERS_UNAVAILABLE";
/// The binding name of `bynk dev`'s agent-inspection registry.
pub const INSPECT_REGISTRY: &str = "__BYNK_INSPECT_REGISTRY";
/// The secret a `bynk migrate sweep` key is set under for the sweep.
pub const MIGRATE_KEY: &str = "BYNK_MIGRATE_KEY";

const fn runtime(file: &'static str) -> Reader {
    Reader {
        dir: ReaderDir::Runtime,
        file,
    }
}

const fn test_runtime(file: &'static str) -> Reader {
    Reader {
        dir: ReaderDir::TestRuntime,
        file,
    }
}

const CLOUDFLARE_BINDING: Reader = Reader {
    dir: ReaderDir::Std,
    file: "cloudflare/cloudflare.binding.ts",
};

/// The generated test main, which emits the readers of the names it reads.
const TEST_MAIN: &str = "bynk-driver::project::tests_emit::test_main";

const fn process(
    name: &'static str,
    set_by: &'static str,
    readers: &'static [Reader],
    emitted_by: Option<&'static str>,
) -> Entry {
    Entry {
        name,
        carrier: Carrier::Process,
        shape: Shape::Exact,
        set_by,
        readers,
        emitted_by,
    }
}

const fn worker(
    name: &'static str,
    carrier: Carrier,
    shape: Shape,
    set_by: &'static str,
    readers: &'static [Reader],
) -> Entry {
    Entry {
        name,
        carrier,
        shape,
        set_by,
        readers,
        emitted_by: None,
    }
}

/// Every protocol name, with who sets it and who reads it.
pub const PROTOCOL: &[Entry] = &[
    process(
        TEST_FORMAT,
        "bynk-driver::test_runner",
        &[runtime("http.ts")],
        Some(TEST_MAIN),
    ),
    process(
        TEST_FILTER,
        "bynk-driver::test_filter",
        &[],
        Some(TEST_MAIN),
    ),
    process(
        TEST_SEED,
        "bynk-driver::test_runner",
        &[test_runtime("property.ts")],
        None,
    ),
    process(
        TEST_SETTLE_MS,
        "bynk-driver::test_runner",
        &[test_runtime("settle.ts")],
        None,
    ),
    process(
        TEST_TARGETS,
        "bynk-driver::test_runner",
        &[test_runtime("remote.ts")],
        None,
    ),
    process(
        TEST_SECRETS,
        "bynk-driver::test_runner",
        &[test_runtime("remote.ts")],
        None,
    ),
    process(
        TEST_RUN_ID,
        "bynk-driver::test_runner",
        &[test_runtime("run_id.ts")],
        None,
    ),
    process(
        TEST_STRICT_WIRE,
        "bynk-driver::test_runner",
        &[runtime("http.ts")],
        None,
    ),
    process(
        SMOKE_DECLARED_FILE,
        "bynk-driver::test_runner",
        &[runtime("http.ts")],
        None,
    ),
    process(
        UPDATE_SNAPSHOTS,
        "bynk-driver::test_runner",
        &[test_runtime("snapshot.ts")],
        None,
    ),
    process(
        SNAPSHOT_REPORT,
        "bynk-driver::test_runner",
        &[],
        Some(TEST_MAIN),
    ),
    worker(
        DEV,
        Carrier::WorkerVar,
        Shape::Exact,
        "bynk::dev::dev_var_args",
        &[CLOUDFLARE_BINDING],
    ),
    worker(
        DEV_ACK_PREFIX,
        Carrier::WorkerVar,
        Shape::Prefix {
            members: &[
                DEV_ACK_AI,
                DEV_ACK_VECTORIZE,
                DEV_ACK_AI_SEARCH,
                DEV_ACK_MTLS,
                DEV_ACK_ANALYTICS_SQL,
            ],
        },
        "bynk::dev::dev_var_args (bynk-project::dev_ack::REMOTE_KINDS)",
        &[CLOUDFLARE_BINDING],
    ),
    worker(
        DEV_INDEX_PREFIX,
        Carrier::WorkerVar,
        Shape::Prefix { members: &[] },
        "bynk-emit::emitter::wrangler",
        &[CLOUDFLARE_BINDING],
    ),
    worker(
        DATASET_PREFIX,
        Carrier::WorkerVar,
        Shape::Prefix { members: &[] },
        "bynk-emit::emitter::wrangler",
        &[CLOUDFLARE_BINDING],
    ),
    worker(
        CALL_FROM_PREFIX,
        Carrier::Secret,
        Shape::Prefix {
            members: &[CALL_FROM_BYNK_DEV],
        },
        "bynk::deploy::secrets and bynk::dev (bynk-check::wire_format::edge_key_from)",
        &[runtime("boundary.ts")],
    ),
    worker(
        CALL_TO_PREFIX,
        Carrier::Secret,
        Shape::Prefix { members: &[] },
        "bynk::deploy::secrets and bynk::dev (bynk-check::wire_format::edge_key_to)",
        &[runtime("boundary.ts")],
    ),
    worker(
        CONTAINERS_UNAVAILABLE,
        Carrier::WorkerVar,
        Shape::Exact,
        "bynk::dev_containers",
        &[runtime("container.ts")],
    ),
    worker(
        INSPECT_REGISTRY,
        Carrier::Binding,
        Shape::Exact,
        "bynk-emit::emitter::wrangler",
        &[runtime("inspect.ts")],
    ),
    Entry {
        name: MIGRATE_KEY,
        carrier: Carrier::Secret,
        shape: Shape::Exact,
        set_by: "bynk::migrate",
        readers: &[],
        emitted_by: Some("bynk-emit::emitter::workers_entry"),
    },
];

/// Every concrete name and prefix the protocol spells: each entry's name, and
/// each prefix's fixed members.
pub fn all_names() -> impl Iterator<Item = &'static str> {
    PROTOCOL.iter().flat_map(|entry| {
        let members: &'static [&'static str] = match entry.shape {
            Shape::Exact => &[],
            Shape::Prefix { members } => members,
        };
        std::iter::once(entry.name).chain(members.iter().copied())
    })
}

/// The entry `token` belongs to: the entry it names, or the prefix entry it
/// starts with (a family member such as `BYNK_DATASET_ANALYTICS_SIGNUPS`).
pub fn entry_of(token: &str) -> Option<&'static Entry> {
    PROTOCOL.iter().find(|entry| match entry.shape {
        Shape::Exact => entry.name == token,
        Shape::Prefix { .. } => token.starts_with(entry.name),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn the_protocol_has_twenty_keys_and_twenty_four_names_a_reader_spells() {
        assert_eq!(PROTOCOL.len(), 20);
        // `BYNK_DEV_ACK_` is a family of five fixed members, so the keys come to
        // 24 names a reader spells. The prefix itself is a token, not a name, and
        // `BYNK_CALL_FROM_BYNK__DEV` is derived from its prefix at run time.
        let spelt = all_names()
            .filter(|n| *n != DEV_ACK_PREFIX && *n != CALL_FROM_BYNK_DEV)
            .count();
        assert_eq!(spelt, 24);
    }

    #[test]
    fn names_are_unique() {
        let mut seen = BTreeSet::new();
        for name in all_names() {
            assert!(seen.insert(name), "{name} is listed twice");
        }
    }

    #[test]
    fn names_are_upper_snake_in_the_bynk_namespace() {
        for name in all_names() {
            let rest = name
                .strip_prefix("__BYNK_")
                .or_else(|| name.strip_prefix("BYNK_"))
                .unwrap_or_else(|| panic!("{name} is outside the BYNK_ namespace"));
            assert!(!rest.is_empty(), "{name}");
            assert!(
                rest.bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'),
                "{name} is not upper snake case"
            );
        }
    }

    #[test]
    fn prefixes_end_in_an_underscore_and_exact_names_do_not() {
        for entry in PROTOCOL {
            match entry.shape {
                Shape::Prefix { members } => {
                    assert!(entry.name.ends_with('_'), "{}", entry.name);
                    for member in members {
                        assert!(member.starts_with(entry.name), "{member}");
                        assert!(member.len() > entry.name.len(), "{member}");
                    }
                }
                Shape::Exact => assert!(!entry.name.ends_with('_'), "{}", entry.name),
            }
        }
    }

    #[test]
    fn every_entry_has_a_reader() {
        for entry in PROTOCOL {
            assert!(
                !entry.readers.is_empty() || entry.emitted_by.is_some(),
                "{} has no reader",
                entry.name
            );
        }
    }

    #[test]
    fn process_variables_are_test_process_names() {
        for entry in PROTOCOL.iter().filter(|e| e.carrier == Carrier::Process) {
            assert_eq!(entry.shape, Shape::Exact, "{}", entry.name);
            assert!(
                entry.name.starts_with("BYNK_TEST_")
                    || entry.name.starts_with("BYNK_SMOKE_")
                    || entry.name.starts_with("BYNK_UPDATE_")
                    || entry.name.starts_with("BYNK_SNAPSHOT_"),
                "{}",
                entry.name
            );
        }
    }

    #[test]
    fn a_name_finds_its_entry_and_a_family_member_its_prefix() {
        assert_eq!(entry_of(TEST_SEED).map(|e| e.name), Some(TEST_SEED));
        assert_eq!(
            entry_of("BYNK_DATASET_ANALYTICS_SIGNUPS").map(|e| e.name),
            Some(DATASET_PREFIX)
        );
        assert_eq!(
            entry_of(DEV_ACK_AI).map(|e| e.name),
            Some(DEV_ACK_PREFIX),
            "an acknowledgement belongs to its family"
        );
        assert_eq!(
            entry_of(CALL_FROM_BYNK_DEV).map(|e| e.name),
            Some(CALL_FROM_PREFIX)
        );
        // `BYNK_DEV` is exact: a longer name is not one of its members.
        assert_eq!(entry_of("BYNK_DEV_COMMAND"), None);
        assert_eq!(entry_of("BYNK_TEST_SEDE"), None);
    }

    #[test]
    fn the_dev_acknowledgements_are_the_ack_family() {
        let kinds: Vec<&str> = crate::dev_ack::REMOTE_KINDS.iter().map(|k| k.var).collect();
        let Some(Shape::Prefix { members }) = entry_of(DEV_ACK_AI).map(|e| e.shape) else {
            panic!("the acknowledgements are a prefix family");
        };
        assert_eq!(kinds, members);
    }

    #[test]
    fn reader_paths_join_the_directory() {
        assert_eq!(
            CLOUDFLARE_BINDING.path(),
            "bynk-std/lib/cloudflare/cloudflare.binding.ts"
        );
        assert_eq!(
            test_runtime("property.ts").path(),
            "bynk-driver/src/project/test_runtime/property.ts"
        );
    }
}
