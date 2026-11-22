//! The message fixture for 1D.3 (#4043, Decision F): what each wrangler operation the deploy's
//! resource kinds run says back, pinned before the operations move behind `CloudflareApi`.
//!
//! Each case runs one operation against a scripted wrangler and records the arguments it was run
//! with and what the operation returned: its value, or its error's text. The scripts cover the
//! shapes a call can fail in (wrangler missing, wrangler failing to start, a non-zero exit with
//! only stderr, only stdout, both or neither, a signal, and a successful exit whose output cannot
//! be read) and the answers each kind treats as success ("already exists", "already gone").
//!
//! `fixtures/cloudflare_messages.txt` is the result. A diff in it is a stop-and-report, never a
//! bless: the typed errors must keep today's text byte for byte.

use super::*;

use std::fmt::{Debug, Display};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// One scripted reply: the exit (a code, or a signal the script sends itself) and both streams.
#[derive(Clone, Copy)]
struct Answer {
    exit: Exit,
    stdout: &'static str,
    stderr: &'static str,
}

#[derive(Clone, Copy)]
enum Exit {
    Code(i32),
    Signal(&'static str),
}

const fn answer(code: i32, stdout: &'static str, stderr: &'static str) -> Answer {
    Answer {
        exit: Exit::Code(code),
        stdout,
        stderr,
    }
}

/// How wrangler is reached in a case.
#[derive(Clone, Copy)]
enum Wrangler {
    /// No wrangler at all (`Provenance::Missing`).
    Missing,
    /// A wrangler path that cannot be started.
    NotStartable,
    /// A wrangler that answers by its arguments: the first pattern (a shell `case` pattern over
    /// `$*`) that matches gives the reply.
    Answers(&'static [(&'static str, Answer)]),
}

/// The failure shapes every operation is run through, with the reply for any arguments.
const SHAPES: &[(&str, Wrangler)] = &[
    ("wrangler missing", Wrangler::Missing),
    ("wrangler does not start", Wrangler::NotStartable),
    (
        "exits 1, stderr only",
        Wrangler::Answers(&[("*", answer(1, "", "it went wrong on stderr\n"))]),
    ),
    (
        "exits 1, stdout only",
        Wrangler::Answers(&[("*", answer(1, "it went wrong on stdout\n", ""))]),
    ),
    (
        "exits 1, both streams",
        Wrangler::Answers(&[(
            "*",
            answer(1, "  and on stdout  \n", "\n  wrong on stderr\n"),
        )]),
    ),
    (
        "exits 3, silent",
        Wrangler::Answers(&[("*", answer(3, "", ""))]),
    ),
    (
        "killed by SIGTERM, silent",
        Wrangler::Answers(&[(
            "*",
            Answer {
                exit: Exit::Signal("TERM"),
                stdout: "",
                stderr: "",
            },
        )]),
    ),
    (
        "exits 0, unreadable output",
        Wrangler::Answers(&[("*", answer(0, "nothing this reads\n", ""))]),
    ),
];

/// A scratch directory per case, with the stand-in and its call log in it.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "bynk-cf-messages-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch { dir }
    }

    /// The wrangler a case runs, written to disk where it is a script.
    fn provenance(&self, wrangler: Wrangler) -> Provenance {
        match wrangler {
            Wrangler::Missing => Provenance::Missing,
            Wrangler::NotStartable => Provenance::Path(self.dir.join("no-such-wrangler")),
            Wrangler::Answers(answers) => {
                let mut script = format!(
                    "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\ncase \"$*\" in\n",
                    self.log().display()
                );
                for (pattern, answer) in answers {
                    let exit = match answer.exit {
                        Exit::Code(code) => format!("exit {code}"),
                        Exit::Signal(signal) => format!("kill -{signal} $$"),
                    };
                    script.push_str(&format!(
                        "  {pattern}) printf '%s' '{}'; printf '%s' '{}' >&2; {exit} ;;\n",
                        answer.stdout, answer.stderr
                    ));
                }
                script.push_str("esac\necho \"unscripted: $*\" >&2\nexit 99\n");
                let path = self.dir.join("wrangler");
                crate::test_support::write_executable(&path, &script);
                Provenance::Path(path)
            }
        }
    }

    fn log(&self) -> PathBuf {
        self.dir.join("calls.log")
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(self.log()).unwrap_or_default()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// An operation's result as the fixture shows it: `ok` and the value, or `error` and the text.
fn shown<T: Debug, E: Display>(result: Result<T, E>) -> String {
    match result {
        Ok(value) => format!("ok {value:?}"),
        Err(e) => format!("error {e}"),
    }
}

/// One operation of the fixture: its name and how to run it from a project root.
type Run = fn(&Provenance, &Path) -> String;

fn decl() -> VectorizeDecl {
    VectorizeDecl {
        binding: "VECTORIZE_DOCS".into(),
        name: "docs".into(),
        dimensions: 768,
        metric: "cosine".into(),
        indexed: vec!["category:string".into()],
        adopt: None,
    }
}

/// Every operation the resource kinds run through wrangler, by the name the fixture gives it.
const OPERATIONS: &[(&str, Run)] = &[
    ("queues create", |p, root| {
        shown(create_queue(p, "jobs", root))
    }),
    ("queues info", |p, root| {
        format!("{:?}", queue_exists(p, "jobs", root))
    }),
    ("queues delete", |p, root| {
        shown(delete_queue(p, "jobs", root))
    }),
    ("kv namespace create", |p, root| {
        shown(create_kv(p, "shop-api", root))
    }),
    ("kv namespace list", |p, root| {
        format!("{:?}", live_kv_namespace_ids(p, root))
    }),
    ("kv namespace delete", |p, root| {
        shown(delete_kv_namespace(p, "0123abcd", root))
    }),
    ("d1 migrations apply (default)", |p, root| {
        shown(apply_d1_migrations(p, root, "DB", "default"))
    }),
    ("d1 migrations apply (--env staging)", |p, root| {
        shown(apply_d1_migrations(p, root, "DB", "staging"))
    }),
    ("d1 create", |p, root| shown(create_d1(p, "shop-db", root))),
    ("r2 bucket create", |p, root| {
        shown(create_r2_bucket(p, "shop-files", None, root))
    }),
    ("r2 bucket create (--jurisdiction eu)", |p, root| {
        shown(create_r2_bucket(p, "shop-files", Some("eu"), root))
    }),
    ("vectorize list", |p, root| {
        shown(index_exists(p, "docs", root))
    }),
    ("vectorize get", |p, root| {
        shown(read_index_config(p, "docs", root))
    }),
    ("vectorize create", |p, root| {
        shown(create_index(p, "docs", &decl(), root))
    }),
    ("vectorize list-metadata-index", |p, root| {
        shown(existing_metadata_indexes(p, "docs", root))
    }),
    ("vectorize create-metadata-index", |p, root| {
        shown(create_metadata_index(p, "docs", "category:string", root))
    }),
    (
        "vectorize create-metadata-index (not name:type)",
        |p, root| shown(create_metadata_index(p, "docs", "category", root)),
    ),
    ("vectorize delete", |p, root| {
        shown(delete_vectorize_index(p, "docs", root))
    }),
];

/// The replies each operation treats specially: its success, and the failures it reads as a
/// success or answers with a message of its own.
const SPECIAL: &[(&str, &str, Wrangler)] = &[
    (
        "queues create",
        "created",
        Wrangler::Answers(&[("*", answer(0, "Created queue jobs.\n", ""))]),
    ),
    (
        "queues create",
        "already exists, on stderr",
        Wrangler::Answers(&[(
            "*",
            answer(
                1,
                "",
                "A queue with this name Already Exists [code: 11009]\n",
            ),
        )]),
    ),
    (
        "queues create",
        "already exists, on stdout",
        Wrangler::Answers(&[("*", answer(1, "Queue already exists\n", ""))]),
    ),
    (
        "queues info",
        "found",
        Wrangler::Answers(&[("*", answer(0, "Queue jobs\n", ""))]),
    ),
    (
        "queues delete",
        "deleted",
        Wrangler::Answers(&[("*", answer(0, "Deleted queue jobs.\n", ""))]),
    ),
    (
        "queues delete",
        "does not exist",
        Wrangler::Answers(&[(
            "*",
            answer(
                1,
                "",
                "Queue \"jobs\" does not exist. To create it, run: wrangler queues create jobs\n",
            ),
        )]),
    ),
    (
        "kv namespace create",
        "TOML snippet",
        Wrangler::Answers(&[(
            "*",
            answer(
                0,
                "[[kv_namespaces]]\nbinding = \"shop_api\"\nid = \"0123abcd\"\n",
                "",
            ),
        )]),
    ),
    (
        "kv namespace create",
        "JSON snippet",
        Wrangler::Answers(&[(
            "*",
            answer(
                0,
                "{\n  \"kv_namespaces\": [\n    { \"binding\": \"shop_api\", \"id\": \"4567ef\" }\n  ]\n}\n",
                "",
            ),
        )]),
    ),
    (
        "kv namespace list",
        "two namespaces",
        Wrangler::Answers(&[(
            "*",
            answer(0, "[{\"id\":\"b2\",\"title\":\"x\"},{\"id\":\"a1\"}]\n", ""),
        )]),
    ),
    (
        "kv namespace delete",
        "deleted",
        Wrangler::Answers(&[("*", answer(0, "Deleted.\n", ""))]),
    ),
    (
        "kv namespace delete",
        "not found, code 10013",
        Wrangler::Answers(&[("*", answer(1, "", "namespace not found [code: 10013]\n"))]),
    ),
    (
        "kv namespace delete",
        "not found, prose",
        Wrangler::Answers(&[("*", answer(1, "", "Namespace Not Found\n"))]),
    ),
    (
        "d1 migrations apply (default)",
        "applied",
        Wrangler::Answers(&[("*", answer(0, "Migrations applied.\n", ""))]),
    ),
    (
        "d1 create",
        "created",
        Wrangler::Answers(&[(
            "*",
            answer(
                0,
                "[[d1_databases]]\nbinding = \"DB\"\ndatabase_name = \"shop-db\"\ndatabase_id = \"0a1b-2c3d\"\n",
                "",
            ),
        )]),
    ),
    (
        "d1 create",
        "created, JSON snippet",
        Wrangler::Answers(&[(
            "*",
            answer(
                0,
                "{ \"d1_databases\": [{ \"database_id\": \"ffee-0011\" }] }\n",
                "",
            ),
        )]),
    ),
    (
        "d1 create",
        "already exists, listed",
        Wrangler::Answers(&[
            (
                "d1\\ create*",
                answer(
                    1,
                    "",
                    "A database with that name already exists [code: 7502]\n",
                ),
            ),
            (
                "d1\\ list*",
                answer(
                    0,
                    "[WARNING] a banner\n[{\"name\":\"other\",\"uuid\":\"9999\"},{\"name\":\"shop-db\",\"uuid\":\"abcd-ef01\"}]\n",
                    "",
                ),
            ),
        ]),
    ),
    (
        "d1 create",
        "already exists, not listed",
        Wrangler::Answers(&[
            (
                "d1\\ create*",
                answer(1, "", "database with that name already exists\n"),
            ),
            ("d1\\ list*", answer(0, "[]\n", "")),
        ]),
    ),
    (
        "d1 create",
        "already exists, listing fails",
        Wrangler::Answers(&[
            (
                "d1\\ create*",
                answer(1, "", "database with that name already exists\n"),
            ),
            ("d1\\ list*", answer(1, "", "could not list\n")),
        ]),
    ),
    (
        "d1 create",
        "already exists, listing silent",
        Wrangler::Answers(&[
            (
                "d1\\ create*",
                answer(1, "", "database with that name already exists\n"),
            ),
            ("d1\\ list*", answer(2, "", "")),
        ]),
    ),
    (
        "r2 bucket create",
        "created",
        Wrangler::Answers(&[("*", answer(0, "Created bucket.\n", ""))]),
    ),
    (
        "r2 bucket create",
        "already exists, and you own it",
        Wrangler::Answers(&[(
            "*",
            answer(
                1,
                "",
                "The bucket you tried to create already exists, and you own it. [code: 10004]\n",
            ),
        )]),
    ),
    (
        "r2 bucket create",
        "already exists, not yours",
        Wrangler::Answers(&[(
            "*",
            answer(1, "", "The bucket already exists and is not available.\n"),
        )]),
    ),
    (
        "vectorize list",
        "listed",
        Wrangler::Answers(&[(
            "*",
            answer(0, "[{\"name\":\"other\"},{\"name\":\"docs\"}]\n", ""),
        )]),
    ),
    (
        "vectorize list",
        "not listed",
        Wrangler::Answers(&[("*", answer(0, "[{\"name\":\"other\"}]\n", ""))]),
    ),
    (
        "vectorize list",
        "no permission, code 10000",
        Wrangler::Answers(&[("*", answer(1, "", "Authentication error [code: 10000]\n"))]),
    ),
    (
        "vectorize get",
        "config at the top level",
        Wrangler::Answers(&[(
            "*",
            answer(0, "{\"dimensions\":768,\"metric\":\"COSINE\"}\n", ""),
        )]),
    ),
    (
        "vectorize get",
        "config nested",
        Wrangler::Answers(&[(
            "*",
            answer(
                0,
                "{\"name\":\"docs\",\"config\":{\"dimensions\":3,\"metric\":\"dot_product\"}}\n",
                "",
            ),
        )]),
    ),
    (
        "vectorize create",
        "created",
        Wrangler::Answers(&[("*", answer(0, "Created index docs.\n", ""))]),
    ),
    (
        "vectorize list-metadata-index",
        "listed",
        Wrangler::Answers(&[(
            "*",
            answer(
                0,
                "{\"metadataIndexes\":[{\"propertyName\":\"category\",\"indexType\":\"String\"}]}\n",
                "",
            ),
        )]),
    ),
    (
        "vectorize create-metadata-index",
        "created",
        Wrangler::Answers(&[("*", answer(0, "Enqueued.\n", ""))]),
    ),
    (
        "vectorize create-metadata-index",
        "already exists",
        Wrangler::Answers(&[(
            "*",
            answer(1, "", "metadata index already exists [code: 40024]\n"),
        )]),
    ),
    (
        "vectorize delete",
        "deleted",
        Wrangler::Answers(&[
            (
                "vectorize\\ list*",
                answer(0, "[{\"name\":\"docs\"}]\n", ""),
            ),
            ("vectorize\\ delete*", answer(0, "Deleted.\n", "")),
        ]),
    ),
    (
        "vectorize delete",
        "already gone",
        Wrangler::Answers(&[("vectorize\\ list*", answer(0, "[]\n", ""))]),
    ),
    (
        "vectorize delete",
        "the delete fails",
        Wrangler::Answers(&[
            (
                "vectorize\\ list*",
                answer(0, "[{\"name\":\"docs\"}]\n", ""),
            ),
            (
                "vectorize\\ delete*",
                answer(1, "", "Authentication error [code: 10000]\n"),
            ),
        ]),
    ),
];

/// One case as the fixture shows it.
fn case(operation: &str, run: Run, shape: &str, wrangler: Wrangler) -> String {
    let scratch = Scratch::new();
    let provenance = scratch.provenance(wrangler);
    let result = run(&provenance, &scratch.dir);
    let mut out = format!("== {operation} · {shape}\n");
    for call in scratch.calls().lines() {
        out.push_str(&format!("   call: {call}\n"));
    }
    for (i, line) in result.lines().enumerate() {
        out.push_str(if i == 0 { "   -> " } else { "      " });
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn messages() -> String {
    let mut out = String::new();
    for (operation, run) in OPERATIONS {
        for (shape, wrangler) in SHAPES {
            out.push_str(&case(operation, *run, shape, *wrangler));
        }
        for (_, shape, wrangler) in SPECIAL.iter().filter(|(op, _, _)| op == operation) {
            out.push_str(&case(operation, *run, shape, *wrangler));
        }
        out.push('\n');
    }
    out
}

#[test]
fn every_wrangler_operation_of_the_resource_kinds_keeps_its_messages() {
    // The stop flag: a SIGTERM another test forwards must not reach these stand-ins.
    let _lock = crate::stop::TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for (operation, _, _) in SPECIAL {
        assert!(
            OPERATIONS.iter().any(|(op, _)| op == operation),
            "a special case names an operation the fixture does not run: {operation}"
        );
    }
    let got = messages();
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/deploy/fixtures/cloudflare_messages.txt");
    if std::env::var_os("BYNK_WRITE_MESSAGE_FIXTURE").is_some() {
        std::fs::write(&path, &got).unwrap();
    }
    let want = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        got == want,
        "the wrangler messages changed. This is a stop-and-report, never a bless (#4043, Decision F).\n--- got ---\n{got}"
    );
}
