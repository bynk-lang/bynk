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
    /// Log what arrived on stdin (`stdin: …`), for the calls that take a value there.
    stdin: bool,
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
        stdin: false,
    }
}

/// [`answer`], logging what the call sent on stdin.
const fn reading(code: i32, stdout: &'static str, stderr: &'static str) -> Answer {
    Answer {
        stdin: true,
        ..answer(code, stdout, stderr)
    }
}

/// How the case's wrangler is scripted.
#[derive(Clone, Copy)]
enum Script {
    /// No wrangler at all (`Provenance::Missing`).
    Missing,
    /// A wrangler path that cannot be started.
    NotStartable,
    /// A wrangler that answers by its arguments: the first pattern (a shell `case` pattern over
    /// `$*`) that matches gives the reply.
    Answers(&'static [(&'static str, Answer)]),
}

/// The failure shapes every operation is run through, with the reply for any arguments.
const SHAPES: &[(&str, Script)] = &[
    ("wrangler missing", Script::Missing),
    ("wrangler does not start", Script::NotStartable),
    (
        "exits 1, stderr only",
        Script::Answers(&[("*", answer(1, "", "it went wrong on stderr\n"))]),
    ),
    (
        "exits 1, stdout only",
        Script::Answers(&[("*", answer(1, "it went wrong on stdout\n", ""))]),
    ),
    (
        "exits 1, both streams",
        Script::Answers(&[(
            "*",
            answer(1, "  and on stdout  \n", "\n  wrong on stderr\n"),
        )]),
    ),
    (
        "exits 3, silent",
        Script::Answers(&[("*", answer(3, "", ""))]),
    ),
    (
        "killed by SIGTERM, silent",
        Script::Answers(&[(
            "*",
            Answer {
                exit: Exit::Signal("TERM"),
                stdout: "",
                stderr: "",
                stdin: false,
            },
        )]),
    ),
    (
        "exits 0, unreadable output",
        Script::Answers(&[("*", answer(0, "nothing this reads\n", ""))]),
    ),
];

/// How a case reaches wrangler.
enum Reached {
    Provenance(Provenance),
    Script(String),
}

/// A scratch directory per case, with the call log in it.
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

    /// The wrangler a case runs: none, one that cannot start, or a script held in memory and
    /// run with `sh -c` (no stand-in is written to disk).
    fn wrangler(&self, wrangler: Script) -> Reached {
        match wrangler {
            Script::Missing => Reached::Provenance(Provenance::Missing),
            Script::NotStartable => {
                Reached::Provenance(Provenance::Path(self.dir.join("no-such-wrangler")))
            }
            Script::Answers(answers) => {
                let mut script = format!(
                    "printf '%s\\n' \"$*\" >> '{}'\ncase \"$*\" in\n",
                    self.log().display()
                );
                for (pattern, answer) in answers {
                    let exit = match answer.exit {
                        Exit::Code(code) => format!("exit {code}"),
                        Exit::Signal(signal) => format!("kill -{signal} $$"),
                    };
                    let stdin = if answer.stdin {
                        format!(
                            "printf 'stdin: %s\\n' \"$(cat)\" >> '{}'; ",
                            self.log().display()
                        )
                    } else {
                        String::new()
                    };
                    script.push_str(&format!(
                        "  {pattern}) {stdin}printf '%s' '{}'; printf '%s' '{}' >&2; {exit} ;;\n",
                        answer.stdout, answer.stderr
                    ));
                }
                script.push_str("esac\necho \"unscripted: $*\" >&2\nexit 99\n");
                Reached::Script(script)
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
type Run = fn(&dyn CloudflareApi, &Path) -> String;

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
    ("queues create", |p, _| shown(create_queue(p, "jobs"))),
    ("queues info", |p, _| {
        format!("{:?}", queue_exists(p, "jobs"))
    }),
    ("queues delete", |p, _| shown(delete_queue(p, "jobs"))),
    ("kv namespace create", |p, _| {
        shown(create_kv(p, "shop-api"))
    }),
    ("kv namespace list", |p, _| {
        format!("{:?}", live_kv_namespace_ids(p))
    }),
    ("kv namespace delete", |p, _| {
        shown(delete_kv_namespace(p, "0123abcd"))
    }),
    ("d1 migrations apply (default)", |p, root| {
        shown(apply_d1_migrations(p, root, "DB", "default"))
    }),
    ("d1 migrations apply (--env staging)", |p, root| {
        shown(apply_d1_migrations(p, root, "DB", "staging"))
    }),
    ("d1 create", |p, _| shown(create_d1(p, "shop-db"))),
    ("r2 bucket create", |p, _| {
        shown(create_r2_bucket(p, "shop-files", None))
    }),
    ("r2 bucket create (--jurisdiction eu)", |p, _| {
        shown(create_r2_bucket(p, "shop-files", Some("eu")))
    }),
    ("vectorize list", |p, _| shown(index_exists(p, "docs"))),
    ("vectorize get", |p, _| shown(read_index_config(p, "docs"))),
    ("vectorize create", |p, _| {
        shown(create_index(p, "docs", &decl()))
    }),
    ("vectorize list-metadata-index", |p, _| {
        shown(existing_metadata_indexes(p, "docs"))
    }),
    ("vectorize create-metadata-index", |p, _| {
        shown(create_metadata_index(p, "docs", "category:string"))
    }),
    ("vectorize create-metadata-index (not name:type)", |p, _| {
        shown(create_metadata_index(p, "docs", "category"))
    }),
    ("vectorize delete", |p, _| {
        shown(delete_vectorize_index(p, "docs"))
    }),
];

/// The replies each operation treats specially: its success, and the failures it reads as a
/// success or answers with a message of its own.
const SPECIAL: &[(&str, &str, Script)] = &[
    (
        "queues create",
        "created",
        Script::Answers(&[("*", answer(0, "Created queue jobs.\n", ""))]),
    ),
    (
        "queues create",
        "already exists, on stderr",
        Script::Answers(&[(
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
        Script::Answers(&[("*", answer(1, "Queue already exists\n", ""))]),
    ),
    (
        "queues info",
        "found",
        Script::Answers(&[("*", answer(0, "Queue jobs\n", ""))]),
    ),
    (
        "queues delete",
        "deleted",
        Script::Answers(&[("*", answer(0, "Deleted queue jobs.\n", ""))]),
    ),
    (
        "queues delete",
        "does not exist",
        Script::Answers(&[(
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
        Script::Answers(&[(
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
        Script::Answers(&[(
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
        Script::Answers(&[(
            "*",
            answer(0, "[{\"id\":\"b2\",\"title\":\"x\"},{\"id\":\"a1\"}]\n", ""),
        )]),
    ),
    (
        "kv namespace delete",
        "deleted",
        Script::Answers(&[("*", answer(0, "Deleted.\n", ""))]),
    ),
    (
        "kv namespace delete",
        "not found, code 10013",
        Script::Answers(&[("*", answer(1, "", "namespace not found [code: 10013]\n"))]),
    ),
    (
        "kv namespace delete",
        "not found, prose",
        Script::Answers(&[("*", answer(1, "", "Namespace Not Found\n"))]),
    ),
    (
        "d1 migrations apply (default)",
        "applied",
        Script::Answers(&[("*", answer(0, "Migrations applied.\n", ""))]),
    ),
    (
        "d1 create",
        "created",
        Script::Answers(&[(
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
        Script::Answers(&[(
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
        Script::Answers(&[
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
        Script::Answers(&[
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
        Script::Answers(&[
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
        Script::Answers(&[
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
        Script::Answers(&[("*", answer(0, "Created bucket.\n", ""))]),
    ),
    (
        "r2 bucket create",
        "already exists, and you own it",
        Script::Answers(&[(
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
        Script::Answers(&[(
            "*",
            answer(1, "", "The bucket already exists and is not available.\n"),
        )]),
    ),
    (
        "vectorize list",
        "listed",
        Script::Answers(&[(
            "*",
            answer(0, "[{\"name\":\"other\"},{\"name\":\"docs\"}]\n", ""),
        )]),
    ),
    (
        "vectorize list",
        "not listed",
        Script::Answers(&[("*", answer(0, "[{\"name\":\"other\"}]\n", ""))]),
    ),
    (
        "vectorize list",
        "no permission, code 10000",
        Script::Answers(&[("*", answer(1, "", "Authentication error [code: 10000]\n"))]),
    ),
    (
        "vectorize get",
        "config at the top level",
        Script::Answers(&[(
            "*",
            answer(0, "{\"dimensions\":768,\"metric\":\"COSINE\"}\n", ""),
        )]),
    ),
    (
        "vectorize get",
        "config nested",
        Script::Answers(&[(
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
        Script::Answers(&[("*", answer(0, "Created index docs.\n", ""))]),
    ),
    (
        "vectorize list-metadata-index",
        "listed",
        Script::Answers(&[(
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
        Script::Answers(&[("*", answer(0, "Enqueued.\n", ""))]),
    ),
    (
        "vectorize create-metadata-index",
        "already exists",
        Script::Answers(&[(
            "*",
            answer(1, "", "metadata index already exists [code: 40024]\n"),
        )]),
    ),
    (
        "vectorize delete",
        "deleted",
        Script::Answers(&[
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
        Script::Answers(&[("vectorize\\ list*", answer(0, "[]\n", ""))]),
    ),
    (
        "vectorize delete",
        "the delete fails",
        Script::Answers(&[
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

/// What an operation runs with: the account operations and the scratch project root.
struct Cx<'a> {
    api: &'a dyn CloudflareApi,
    root: &'a Path,
}

/// One operation of the rest of wrangler: its name and how to run it.
type RunB = fn(&Cx<'_>) -> String;

/// A push's or a deploy step's result: `ok` and the value, or the failure's exit code and text.
fn failed<T: Debug>(result: Result<T, DeployFailure>) -> String {
    match result {
        Ok(value) => format!("ok {value:?}"),
        Err(f) => format!("exit {} {}", f.code, f.message),
    }
}

/// The rest of wrangler (1D.3b, #4043): the push, the triggers, the account reads, the secrets,
/// `rollback` and the migration sweep's secrets, pinned before they move.
const OPERATIONS_B: &[(&str, RunB)] = &[
    ("deploy", |c| {
        failed(push(c.api, c.root, "api", &[], "default", (None, false)))
    }),
    ("deploy (--env staging, with arguments)", |c| {
        failed(push(
            c.api,
            c.root,
            "api",
            &["--var".into(), "A:1".into()],
            "staging",
            (None, false),
        ))
    }),
    ("deploy (with a secrets file)", |c| {
        let file = c.root.join("secrets.json");
        std::fs::write(&file, "{}").unwrap();
        failed(push(
            c.api,
            c.root,
            "api",
            &[],
            "default",
            (Some(&file), false),
        ))
    }),
    ("triggers deploy", |c| {
        failed(deploy_forgiven_triggers(c.api, c.root, "api", "staging"))
    }),
    ("whoami", |c| format!("{:?}", whoami(c.api))),
    ("deployments list", |c| {
        format!("{:?}", worker_presence(c.api, "billing"))
    }),
    ("secret list (default)", |c| {
        format!("{:?}", list_secrets(c.api, c.root, "default"))
    }),
    ("secret list (--env staging)", |c| {
        format!("{:?}", list_secrets(c.api, c.root, "staging"))
    }),
    ("secret put", |c| {
        shown(set_secret(c.api, c.root, "TOKEN", "s3cret", "staging"))
    }),
    ("rollback", |c| {
        let account = crate::deploy::revert::LiveAccount {
            rest: crate::migrate::live::tests::rest("http://127.0.0.1:9".into()),
            api: c.api,
        };
        shown(crate::deploy::revert::Account::rollback(
            &account, "shop-api", "v-123",
        ))
    }),
    ("secret put (by name, for the sweep)", |c| {
        let secrets = crate::migrate::live::WranglerSecrets {
            api: c.api,
            scratch: c.root.join("sweep"),
        };
        shown(crate::migrate::sweep::Secrets::put(
            &secrets,
            "shop-api",
            "MIGRATE_KEY",
            "k3y",
        ))
    }),
    ("secret delete (by name, for the sweep)", |c| {
        let secrets = crate::migrate::live::WranglerSecrets {
            api: c.api,
            scratch: c.root.join("sweep"),
        };
        shown(crate::migrate::sweep::Secrets::delete(
            &secrets,
            "shop-api",
            "MIGRATE_KEY",
        ))
    }),
];

/// The rest of wrangler's own answers.
const SPECIAL_B: &[(&str, &str, Script)] = &[
    (
        "deploy",
        "interrupted by SIGINT",
        Script::Answers(&[(
            "*",
            Answer {
                exit: Exit::Signal("INT"),
                stdout: "",
                stderr: "",
                stdin: false,
            },
        )]),
    ),
    (
        "deployments list",
        "listed",
        Script::Answers(&[("*", answer(0, "[]\n", ""))]),
    ),
    (
        "deployments list",
        "not found, code 10007",
        Script::Answers(&[(
            "*",
            answer(1, "", "This Worker does not exist [code: 10007]\n"),
        )]),
    ),
    (
        "deployments list",
        "not found, code 10090",
        Script::Answers(&[("*", answer(1, "", "legacy env [code: 10090]\n"))]),
    ),
    (
        "secret list (default)",
        "two secrets",
        Script::Answers(&[(
            "*",
            answer(
                0,
                "[{\"name\":\"TOKEN\",\"type\":\"secret_text\"},{\"name\":\"KEY\"}]\n",
                "",
            ),
        )]),
    ),
    (
        "secret put",
        "set, value on stdin",
        Script::Answers(&[("*", reading(0, "Success!\n", ""))]),
    ),
    (
        "secret put",
        "fails, value on stdin",
        Script::Answers(&[("*", reading(1, "", "Authentication error [code: 10000]\n"))]),
    ),
    (
        "rollback",
        "rolled back",
        Script::Answers(&[("*", answer(0, "Rolled back.\n", ""))]),
    ),
    (
        "rollback",
        "fails, stderr and stdout",
        Script::Answers(&[("*", answer(1, "  on stdout\n", "  on stderr\n"))]),
    ),
    (
        "secret put (by name, for the sweep)",
        "set, value on stdin",
        Script::Answers(&[("*", reading(0, "Success!\n", ""))]),
    ),
    (
        "secret delete (by name, for the sweep)",
        "deleted, nothing on stdin",
        Script::Answers(&[("*", reading(0, "Deleted.\n", ""))]),
    ),
];

/// One case as the fixture shows it.
fn case(operation: &str, run: Run, shape: &str, wrangler: Script) -> String {
    case_with(operation, shape, wrangler, |cx| run(cx.api, cx.root))
}

/// One case of the rest of wrangler.
fn case_b(operation: &str, run: RunB, shape: &str, wrangler: Script) -> String {
    case_with(operation, shape, wrangler, run)
}

fn case_with(
    operation: &str,
    shape: &str,
    wrangler: Script,
    run: impl FnOnce(&Cx<'_>) -> String,
) -> String {
    let scratch = Scratch::new();
    let reached = scratch.wrangler(wrangler);
    let api = match &reached {
        Reached::Provenance(provenance) => Wrangler::new(provenance, &scratch.dir),
        Reached::Script(script) => Wrangler::scripted(script, &scratch.dir),
    };
    let result = run(&Cx {
        api: &api,
        root: &scratch.dir,
    });
    let at = scratch.dir.display().to_string();
    let result = result.replace(&at, "<root>");
    let mut out = format!("== {operation} · {shape}\n");
    for call in scratch.calls().replace(&at, "<root>").lines() {
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
    for (operation, run) in OPERATIONS_B {
        for (shape, wrangler) in SHAPES {
            out.push_str(&case_b(operation, *run, shape, *wrangler));
        }
        for (_, shape, wrangler) in SPECIAL_B.iter().filter(|(op, _, _)| op == operation) {
            out.push_str(&case_b(operation, *run, shape, *wrangler));
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
    for (operation, _, _) in SPECIAL_B {
        assert!(
            OPERATIONS_B.iter().any(|(op, _)| op == operation),
            "a special case names an operation the fixture does not run: {operation}"
        );
    }
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
