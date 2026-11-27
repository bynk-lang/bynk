//! The message fixture for 1D.3 (#4043, Decision F): what each wrangler operation the deploy's
//! resource kinds run says back, pinned before the operations move behind `CloudflareApi`.
//!
//! Each case runs one operation against a scripted wrangler and records the arguments it was run
//! with and what the operation returned: its value, or its error's text. The scripts cover the
//! shapes a call can fail in (wrangler missing, wrangler failing to start, a non-zero exit with
//! only stderr, only stdout, both or neither, a signal, and a successful exit whose output cannot
//! be read) and the answers each kind treats as success ("already exists", "already gone").
//!
//! A kind that moves to REST (1D.4, #4056 Decision G) leaves the wrangler cases and gains REST
//! ones at the end: each operation against a stub that answers each of [`REST_SHAPES`], and its
//! own successes.
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
    ///
    /// `drains`: the call pipes a value to wrangler's stdin, so the script reads it first, as
    /// wrangler does. A script that exited without reading would race `bynk`'s write, and the
    /// write could fail with a broken pipe instead of the shape the case scripts.
    fn wrangler(&self, wrangler: Script, drains: bool) -> Reached {
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
                    } else if drains {
                        "cat > /dev/null; ".to_string()
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
    ("d1 migrations apply (default)", |p, root| {
        shown(apply_d1_migrations(p, root, "DB", "default"))
    }),
    ("d1 migrations apply (--env staging)", |p, root| {
        shown(apply_d1_migrations(p, root, "DB", "staging"))
    }),
];

/// The replies each operation treats specially: its success, and the failures it reads as a
/// success or answers with a message of its own.
const SPECIAL: &[(&str, &str, Script)] = &[(
    "d1 migrations apply (default)",
    "applied",
    Script::Answers(&[("*", answer(0, "Migrations applied.\n", ""))]),
)];

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
        // `Unknown` carries the typed error since 1D.3c; the fixture pinned its text.
        match worker_presence(c.api, "billing") {
            WorkerPresence::Unknown(said) => format!("Unknown({:?})", said.to_string()),
            known => format!("{known:?}"),
        }
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
/// The operations that pipe a value to wrangler's stdin (`Wrangler::feed`); their stand-ins
/// read it, as wrangler does. Every other call's stdin is inherited or closed, so a stand-in
/// that read it could block.
const FEEDS_STDIN: &[&str] = &[
    "secret put",
    "secret put (by name, for the sweep)",
    "secret delete (by name, for the sweep)",
];

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
    let reached = scratch.wrangler(wrangler, FEEDS_STDIN.contains(&operation));
    let api = match &reached {
        Reached::Provenance(provenance) => Wrangler::new(provenance, &scratch.dir),
        Reached::Script(script) => Wrangler::scripted(script, &scratch.dir),
    };
    let api = crate::cloudflare::Account::new(api);
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
    out.push_str(&rest_kind_messages());
    out
}

// ---- 1D.4 (#4056 Decision G): the moved kinds' operations, over REST ----

/// The operations that moved to REST, by the name the fixture gives them: KV (1D.4a), D1 and R2
/// (1D.4b), Vectorize (1D.4c) and queues (1D.4d).
const REST_OPERATIONS: &[(&str, Run)] = &[
    ("KV create (REST)", |p, _| shown(create_kv(p, "shop-api"))),
    ("KV list (REST)", |p, _| {
        format!("{:?}", live_kv_namespace_ids(p))
    }),
    ("KV delete (REST)", |p, _| {
        shown(delete_kv_namespace(p, "0123abcd"))
    }),
    ("D1 create (REST)", |p, _| {
        shown(create_d1(p, "shop-db", None))
    }),
    ("D1 create (REST, jurisdiction eu)", |p, _| {
        shown(create_d1(p, "shop-db", Some("eu")))
    }),
    ("R2 create (REST)", |p, _| {
        shown(create_r2_bucket(p, "shop-files", None))
    }),
    ("R2 create (REST, jurisdiction eu)", |p, _| {
        shown(create_r2_bucket(p, "shop-files", Some("eu")))
    }),
    ("Vectorize list (REST)", |p, _| {
        shown(index_exists(p, "docs"))
    }),
    ("Vectorize get (REST)", |p, _| {
        shown(read_index_config(p, "docs"))
    }),
    ("Vectorize create (REST)", |p, _| {
        shown(create_index(p, "docs", &decl()))
    }),
    ("Vectorize list metadata indexes (REST)", |p, _| {
        shown(existing_metadata_indexes(p, "docs"))
    }),
    ("Vectorize create metadata index (REST)", |p, _| {
        shown(create_metadata_index(p, "docs", "category:string"))
    }),
    ("Vectorize delete (REST)", |p, _| {
        shown(delete_vectorize_index(p, "docs"))
    }),
    ("Queues create (REST)", |p, _| {
        shown(create_queue(p, "jobs"))
    }),
    ("Queues find (REST)", |p, _| {
        format!("{:?}", queue_exists(p, "jobs"))
    }),
    ("Queues delete (REST)", |p, _| {
        // `shop-jobs` and `shop-mail` are the project's own Workers; any other consumer is
        // refused (#4601).
        let own = BTreeSet::from(["shop-jobs".to_string(), "shop-mail".to_string()]);
        shown(delete_queue(p, "jobs", &own))
    }),
];

/// The answers each moved operation treats specially: its success, and a failure it reads as one.
/// Answers a stub gives in turn: a status and a body each.
type Answers = &'static [(u16, &'static str)];

/// The calls a stub saw, read once it has served them.
type Seen = Box<dyn FnOnce() -> Vec<String>>;

/// Each operation's own answers. A consumed queue's consumer is removed before the queue
/// (#4601): Cloudflare refuses to delete a queue a Worker consumes with 400 `11005`.
const REST_SPECIAL: &[(&str, &str, Answers)] = &[
    (
        "KV create (REST)",
        "created",
        &[(
            200,
            r#"{"success":true,"errors":[],"result":{"id":"0123abcd","title":"shop-api"}}"#,
        )],
    ),
    (
        "KV create (REST)",
        "title taken, code 10014",
        &[(
            400,
            r#"{"success":false,"errors":[{"code":10014,"message":"a namespace with this account ID and title already exists"}],"result":null}"#,
        )],
    ),
    (
        "KV list (REST)",
        "two namespaces, one page",
        &[(
            200,
            r#"{"success":true,"result":[{"id":"b2","title":"x"},{"id":"a1","title":"y"}],"result_info":{"page":1,"per_page":100,"count":2,"total_count":2,"total_pages":1}}"#,
        )],
    ),
    (
        "KV delete (REST)",
        "deleted",
        &[(200, r#"{"success":true,"errors":[],"result":null}"#)],
    ),
    (
        "KV delete (REST)",
        "not found, code 10013",
        &[(
            404,
            r#"{"success":false,"errors":[{"code":10013,"message":"delete namespace: 'namespace not found'"}],"result":null}"#,
        )],
    ),
    (
        "D1 create (REST)",
        "created",
        &[(
            200,
            r#"{"success":true,"errors":[],"result":{"uuid":"0a1b2c3d-0000-4000-8000-000000000000","name":"shop-db"}}"#,
        )],
    ),
    (
        "D1 create (REST)",
        "refused, the name is the account's: adopted",
        &[
            (
                400,
                r#"{"success":false,"errors":[{"code":7502,"message":"A database with that name already exists"}]}"#,
            ),
            (
                200,
                r#"{"success":true,"result":[{"name":"shop-db-old","uuid":"9999"},{"name":"shop-db","uuid":"abcd-ef01"}],"result_info":{"page":1,"per_page":100,"count":2}}"#,
            ),
        ],
    ),
    (
        "D1 create (REST)",
        "refused, no database of exactly that name",
        &[
            (
                400,
                r#"{"success":false,"errors":[{"code":7502,"message":"A database with that name already exists"}]}"#,
            ),
            (
                200,
                r#"{"success":true,"result":[{"name":"shop-db-old","uuid":"9999"}],"result_info":{"page":1,"per_page":100,"count":1}}"#,
            ),
        ],
    ),
    (
        "R2 create (REST)",
        "created",
        &[(
            200,
            r#"{"success":true,"errors":[],"result":{"name":"shop-files"}}"#,
        )],
    ),
    (
        "R2 create (REST)",
        "conflict, the account's own bucket: accepted",
        &[
            (
                409,
                r#"{"success":false,"errors":[{"code":10073,"message":"Bucket name already exists."}]}"#,
            ),
            (200, r#"{"success":true,"result":{"name":"shop-files"}}"#),
        ],
    ),
    (
        "R2 create (REST)",
        "conflict, another account's bucket",
        &[
            (
                409,
                r#"{"success":false,"errors":[{"code":10073,"message":"Bucket name already exists."}]}"#,
            ),
            (
                404,
                r#"{"success":false,"errors":[{"code":10006,"message":"The specified bucket does not exist."}]}"#,
            ),
        ],
    ),
    (
        "Vectorize list (REST)",
        "listed",
        &[(
            200,
            r#"{"success":true,"result":[{"name":"other","config":{"dimensions":3,"metric":"cosine"}},{"name":"docs","config":{"dimensions":768,"metric":"cosine"}}]}"#,
        )],
    ),
    (
        "Vectorize list (REST)",
        "not listed",
        &[(200, r#"{"success":true,"result":[{"name":"other"}]}"#)],
    ),
    (
        "Vectorize get (REST)",
        "config, dot-product",
        &[(
            200,
            r#"{"success":true,"result":{"name":"docs","config":{"dimensions":3,"metric":"dot-product"}}}"#,
        )],
    ),
    (
        "Vectorize create (REST)",
        "created",
        &[(
            200,
            r#"{"success":true,"result":{"name":"docs","config":{"dimensions":768,"metric":"cosine"}}}"#,
        )],
    ),
    (
        "Vectorize list metadata indexes (REST)",
        "listed",
        &[(
            200,
            r#"{"success":true,"result":{"metadataIndexes":[{"propertyName":"category","indexType":"String"}]}}"#,
        )],
    ),
    (
        "Vectorize create metadata index (REST)",
        "enqueued",
        &[(
            200,
            r#"{"success":true,"result":{"mutationId":"182bd5e5-6e1a-4fe4-a799-aa6d9a6ab26e"}}"#,
        )],
    ),
    (
        "Vectorize create metadata index (REST)",
        "refused, left to the listing wait",
        &[(
            409,
            r#"{"success":false,"errors":[{"code":40004,"message":"metadata index already exists"}]}"#,
        )],
    ),
    (
        "Vectorize delete (REST)",
        "deleted",
        &[
            (200, r#"{"success":true,"result":[{"name":"docs"}]}"#),
            (200, r#"{"success":true,"result":{}}"#),
        ],
    ),
    (
        "Vectorize delete (REST)",
        "already gone",
        &[(200, r#"{"success":true,"result":[]}"#)],
    ),
    (
        "Vectorize delete (REST)",
        "the delete is denied",
        &[
            (200, r#"{"success":true,"result":[{"name":"docs"}]}"#),
            (
                403,
                r#"{"success":false,"errors":[{"code":10000,"message":"Authentication error"}]}"#,
            ),
        ],
    ),
    (
        "Queues create (REST)",
        "created",
        &[(
            200,
            r#"{"success":true,"result":{"queue_id":"4e5f6a7b","queue_name":"jobs"}}"#,
        )],
    ),
    (
        "Queues create (REST)",
        "refused, the queue is listed: a race lost",
        &[
            (
                409,
                r#"{"success":false,"errors":[{"code":11009,"message":"A queue with this name already exists"}]}"#,
            ),
            (
                200,
                r#"{"success":true,"result":[{"queue_id":"0a1b2c3d","queue_name":"jobs-old"},{"queue_id":"4e5f6a7b","queue_name":"jobs"}],"result_info":{"page":1,"per_page":100,"total_pages":1}}"#,
            ),
        ],
    ),
    (
        "Queues create (REST)",
        "refused, no queue of exactly that name",
        &[
            (
                400,
                r#"{"success":false,"errors":[{"code":11009,"message":"A queue with this name already exists"}]}"#,
            ),
            (
                200,
                r#"{"success":true,"result":[{"queue_id":"0a1b2c3d","queue_name":"jobs-old"}],"result_info":{"page":1,"per_page":100,"total_pages":1}}"#,
            ),
        ],
    ),
    (
        "Queues find (REST)",
        "listed",
        &[(
            200,
            r#"{"success":true,"result":[{"queue_id":"0a1b2c3d","queue_name":"jobs-old"},{"queue_id":"4e5f6a7b","queue_name":"jobs"}],"result_info":{"page":1,"per_page":100,"total_pages":1}}"#,
        )],
    ),
    (
        "Queues find (REST)",
        "not listed",
        &[(
            200,
            r#"{"success":true,"result":[{"queue_id":"0a1b2c3d","queue_name":"jobs-old"}],"result_info":{"page":1,"per_page":100,"total_pages":1}}"#,
        )],
    ),
    (
        "Queues delete (REST)",
        "deleted",
        &[
            (
                200,
                r#"{"success":true,"result":[{"queue_id":"0a1b2c3d","queue_name":"jobs-old"},{"queue_id":"4e5f6a7b","queue_name":"jobs"}],"result_info":{"page":1,"per_page":100,"total_pages":1}}"#,
            ),
            (200, r#"{"success":true,"result":[]}"#),
            (200, r#"{"success":true,"errors":[],"result":null}"#),
        ],
    ),
    (
        "Queues delete (REST)",
        "consumed: the consumer is removed first",
        &[
            (
                200,
                r#"{"success":true,"result":[{"queue_id":"0a1b2c3d","queue_name":"jobs-old"},{"queue_id":"4e5f6a7b","queue_name":"jobs"}],"result_info":{"page":1,"per_page":100,"total_pages":1}}"#,
            ),
            (
                200,
                r#"{"success":true,"result":[{"consumer_id":"9f8e7d","script":"shop-jobs","type":"worker"}]}"#,
            ),
            (200, r#"{"success":true,"result":null}"#),
            (200, r#"{"success":true,"errors":[],"result":null}"#),
        ],
    ),
    (
        "Queues delete (REST)",
        "consumer cannot be removed",
        &[
            (
                200,
                r#"{"success":true,"result":[{"queue_id":"0a1b2c3d","queue_name":"jobs-old"},{"queue_id":"4e5f6a7b","queue_name":"jobs"}],"result_info":{"page":1,"per_page":100,"total_pages":1}}"#,
            ),
            (
                200,
                r#"{"success":true,"result":[{"consumer_id":"9f8e7d","script":"shop-jobs","type":"worker"}]}"#,
            ),
            (
                403,
                r#"{"success":false,"errors":[{"code":10000,"message":"Authentication error"}]}"#,
            ),
        ],
    ),
    (
        "Queues delete (REST)",
        "consumer already removed, answered 404",
        &[
            (
                200,
                r#"{"success":true,"result":[{"queue_id":"0a1b2c3d","queue_name":"jobs-old"},{"queue_id":"4e5f6a7b","queue_name":"jobs"}],"result_info":{"page":1,"per_page":100,"total_pages":1}}"#,
            ),
            (
                200,
                r#"{"success":true,"result":[{"consumer_id":"9f8e7d","script":"shop-jobs","type":"worker"}]}"#,
            ),
            (
                404,
                r#"{"success":false,"errors":[{"code":10000,"message":"Consumer not found"}]}"#,
            ),
            (200, r#"{"success":true,"errors":[],"result":null}"#),
        ],
    ),
    (
        "Queues delete (REST)",
        "queue gone between the listing and the consumer delete, code 10104",
        &[
            (
                200,
                r#"{"success":true,"result":[{"queue_id":"0a1b2c3d","queue_name":"jobs-old"},{"queue_id":"4e5f6a7b","queue_name":"jobs"}],"result_info":{"page":1,"per_page":100,"total_pages":1}}"#,
            ),
            (
                200,
                r#"{"success":true,"result":[{"consumer_id":"9f8e7d","script":"shop-jobs","type":"worker"}]}"#,
            ),
            (
                404,
                r#"{"success":false,"errors":[{"code":10104,"message":"Queue does not exist"}]}"#,
            ),
            (
                404,
                r#"{"success":false,"errors":[{"code":10104,"message":"Queue does not exist"}]}"#,
            ),
        ],
    ),
    (
        "Queues delete (REST)",
        "second own consumer cannot be detached",
        &[
            (
                200,
                r#"{"success":true,"result":[{"queue_id":"0a1b2c3d","queue_name":"jobs-old"},{"queue_id":"4e5f6a7b","queue_name":"jobs"}],"result_info":{"page":1,"per_page":100,"total_pages":1}}"#,
            ),
            (
                200,
                r#"{"success":true,"result":[{"consumer_id":"9f8e7d","script":"shop-jobs","type":"worker"},{"consumer_id":"1a2b3c","script":"shop-mail","type":"worker"}]}"#,
            ),
            (200, r#"{"success":true,"result":null}"#),
            (
                403,
                r#"{"success":false,"errors":[{"code":10000,"message":"Authentication error"}]}"#,
            ),
        ],
    ),
    (
        "Queues delete (REST)",
        "an HTTP pull consumer refuses the delete, nothing removed",
        &[
            (
                200,
                r#"{"success":true,"result":[{"queue_id":"0a1b2c3d","queue_name":"jobs-old"},{"queue_id":"4e5f6a7b","queue_name":"jobs"}],"result_info":{"page":1,"per_page":100,"total_pages":1}}"#,
            ),
            (
                200,
                r#"{"success":true,"result":[{"consumer_id":"9f8e7d","script":"shop-jobs","type":"worker"},{"consumer_id":"1a2b3c","type":"http_pull"}]}"#,
            ),
        ],
    ),
    (
        "Queues delete (REST)",
        "a foreign Worker consumer refuses the delete, nothing removed",
        &[
            (
                200,
                r#"{"success":true,"result":[{"queue_id":"0a1b2c3d","queue_name":"jobs-old"},{"queue_id":"4e5f6a7b","queue_name":"jobs"}],"result_info":{"page":1,"per_page":100,"total_pages":1}}"#,
            ),
            (
                200,
                r#"{"success":true,"result":[{"consumer_id":"9f8e7d","script":"billing","type":"worker"}]}"#,
            ),
        ],
    ),
    (
        "Queues delete (REST)",
        "already gone",
        &[(
            200,
            r#"{"success":true,"result":[{"queue_id":"0a1b2c3d","queue_name":"jobs-old"}],"result_info":{"page":1,"per_page":100,"total_pages":1}}"#,
        )],
    ),
    (
        "Queues delete (REST)",
        "not found, code 10104",
        &[
            (
                200,
                r#"{"success":true,"result":[{"queue_id":"0a1b2c3d","queue_name":"jobs-old"},{"queue_id":"4e5f6a7b","queue_name":"jobs"}],"result_info":{"page":1,"per_page":100,"total_pages":1}}"#,
            ),
            (200, r#"{"success":true,"result":[]}"#),
            (
                404,
                r#"{"success":false,"errors":[{"code":10104,"message":"Queue does not exist"}]}"#,
            ),
        ],
    ),
];

/// One moved operation's case: its requests, and what it returned. One answer logs the request
/// with its body; a sequence logs each request's method and path.
fn rest_case(
    operation: &str,
    run: Run,
    shape: &str,
    answers: &'static [(u16, &'static str)],
) -> String {
    let (base, seen): (String, Option<Seen>) = match answers {
        // A port nothing listens on: the connection is refused.
        [] => ("http://127.0.0.1:9".to_string(), None),
        [(status, body)] => {
            let (base, seen) = answering(*status, body);
            (base, Some(Box::new(move || vec![seen.join().unwrap()])))
        }
        _ => {
            let (base, seen) = crate::cloudflare::rest::tests::answering_in_turn(answers);
            (base, Some(Box::new(move || seen.join().unwrap())))
        }
    };
    let scratch = Scratch::new();
    // A wrangler that fails loudly: a moved operation that reached it would say so.
    let api = crate::cloudflare::Account::with_rest(
        Wrangler::scripted("echo \"wrangler ran: $*\" >&2; exit 1", &scratch.dir),
        crate::migrate::live::tests::rest(base.clone()),
    );
    let result = run(&api, &scratch.dir);
    let mut out = format!("== {operation} · {shape}\n");
    if let Some(seen) = seen {
        for request in seen() {
            out.push_str(&format!("   request: {request}\n"));
        }
    }
    let result = os_error_number(&result.replace(&base, "<base>"));
    for (i, line) in result.lines().enumerate() {
        out.push_str(if i == 0 { "   -> " } else { "      " });
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Moved operations refused before any request is sent: each runs once, against no stub.
const REST_UNSENT: &[(&str, Run)] = &[(
    "Vectorize create metadata index (REST, not name:type)",
    |p, _| shown(create_metadata_index(p, "docs", "category")),
)];

fn rest_kind_messages() -> String {
    let mut out = String::new();
    for (operation, run) in REST_UNSENT {
        out.push_str(&rest_case(operation, *run, "nothing sent", &[]));
        out.push('\n');
    }
    for (operation, run) in REST_OPERATIONS {
        for (shape, answer) in REST_SHAPES {
            let answers: &'static [(u16, &'static str)] = match answer {
                None => &[],
                Some(answer) => std::slice::from_ref(answer),
            };
            out.push_str(&rest_case(operation, *run, shape, answers));
        }
        for (_, shape, answers) in REST_SPECIAL.iter().filter(|(op, ..)| op == operation) {
            out.push_str(&rest_case(operation, *run, shape, answers));
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
    for (operation, ..) in REST_SPECIAL {
        assert!(
            REST_OPERATIONS.iter().any(|(op, _)| op == operation),
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

// ---- 1D.3c (#4043): the REST client, pinned before it moves ----

/// A one-request HTTP stub answering `status` and `body`, and the request line it saw (method,
/// path, and the `cf-r2-jurisdiction` header when sent).
fn answering(status: u16, body: &'static str) -> (String, std::thread::JoinHandle<String>) {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let mut request = line
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ");
        let mut length = 0usize;
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                break;
            }
            let lower = header.to_ascii_lowercase();
            if let Some(n) = lower.strip_prefix("content-length:") {
                length = n.trim().parse().unwrap_or(0);
            }
            if lower.starts_with("cf-r2-jurisdiction:") {
                request.push_str(&format!(" [{}]", header.trim()));
            }
        }
        let mut sent = vec![0u8; length];
        let _ = reader.read_exact(&mut sent);
        if length > 0 {
            request.push_str(&format!(" {}", String::from_utf8_lossy(&sent)));
        }
        let mut stream = stream;
        let reason = if status == 200 { "OK" } else { "Error" };
        let _ = write!(
            stream,
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        request
    });
    (base, seen)
}

/// The shapes a REST call is answered in.
const REST_SHAPES: &[(&str, Option<(u16, &str)>)] = &[
    ("unreachable", None),
    (
        "200, success",
        Some((200, r#"{"success":true,"result":{"id":"x"}}"#)),
    ),
    (
        "200, success false",
        Some((
            200,
            r#"{"success":false,"errors":[{"code":7003,"message":"No route"}]}"#,
        )),
    ),
    (
        "404 with errors",
        Some((
            404,
            r#"{"success":false,"errors":[{"code":10007,"message":"not found"}]}"#,
        )),
    ),
    (
        "403 with errors",
        Some((
            403,
            r#"{"success":false,"errors":[{"code":10000,"message":"Authentication error"}]}"#,
        )),
    ),
    ("500 without errors", Some((500, r#"{"success":false}"#))),
    ("200, unreadable", Some((200, "<html>not json</html>"))),
];

type RestRun =
    fn(&crate::cloudflare::Rest) -> Result<serde_json::Value, crate::cloudflare::RestError>;

/// Every request shape the REST client sends.
const REST_METHODS: &[(&str, RestRun)] = &[
    ("GET", |r| r.get("/workers/scripts/s/versions")),
    ("GET with a header", |r| {
        r.get_with_header("/r2/buckets", ("cf-r2-jurisdiction", "eu"))
    }),
    ("GET outside the account", |r| {
        r.get_global("/zones?name=example.com")
    }),
    ("POST", |r| {
        r.post(
            "/d1/database/d/query",
            &serde_json::json!({"sql": "SELECT 1"}),
            crate::cloudflare::Idempotent::Yes,
        )
    }),
    ("PUT", |r| {
        r.put("/hyperdrive/configs/h", &serde_json::json!({"name": "h"}))
    }),
    ("DELETE", |r| r.delete("/hyperdrive/configs/h")),
];

/// `text` with any `(os error N)` written `(os error N)` literally.
fn os_error_number(text: &str) -> String {
    match text.find("(os error ") {
        Some(at) => match text[at..].find(')') {
            Some(end) => format!("{}(os error N){}", &text[..at], &text[at + end + 1..]),
            None => text.to_string(),
        },
        None => text.to_string(),
    }
}

fn rest_messages() -> String {
    let mut out = String::new();
    for (method, run) in REST_METHODS {
        for (shape, answer) in REST_SHAPES {
            let (base, seen) = match answer {
                // A port nothing listens on: the connection is refused.
                None => ("http://127.0.0.1:9".to_string(), None),
                Some((status, body)) => {
                    let (base, seen) = answering(*status, body);
                    (base, Some(seen))
                }
            };
            let rest = crate::migrate::live::tests::rest(base.clone());
            let result = run(&rest);
            out.push_str(&format!("== REST {method} · {shape}\n"));
            if let Some(seen) = seen {
                out.push_str(&format!("   request: {}\n", seen.join().unwrap()));
            }
            let result = match result {
                Ok(value) => format!("ok {value}"),
                // The OS's number for a refused connection differs (111 on Linux, 61 on macOS).
                Err(e) => format!(
                    "error {}",
                    os_error_number(&e.to_string().replace(&base, "<base>"))
                ),
            };
            for (i, line) in result.lines().enumerate() {
                out.push_str(if i == 0 { "   -> " } else { "      " });
                out.push_str(line);
                out.push('\n');
            }
        }
        out.push('\n');
    }
    out
}

#[test]
fn the_rest_client_keeps_its_messages() {
    let got = rest_messages();
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/deploy/fixtures/cloudflare_rest_messages.txt");
    if std::env::var_os("BYNK_WRITE_MESSAGE_FIXTURE").is_some() {
        std::fs::write(&path, &got).unwrap();
    }
    let want = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        got == want,
        "the REST client's messages changed. This is a stop-and-report, never a bless (#4043, Decision F).\n--- got ---\n{got}"
    );
}

/// The old destinations error's shape, for its pinned debug form.
#[derive(Debug)]
#[allow(dead_code)]
struct Failed(String);

/// What each REST consumer says when neither credential is set: the run's environment, so this
/// is checked only where neither is set (CI, and a shell without them).
#[test]
fn each_rest_consumer_names_its_purpose_when_credentials_are_missing() {
    if ["CLOUDFLARE_API_TOKEN", "CLOUDFLARE_ACCOUNT_ID"]
        .iter()
        .any(|v| std::env::var_os(v).is_some())
    {
        eprintln!("skipped: a Cloudflare credential is set in this environment");
        return;
    }
    let mut got = String::new();
    let mut said = |who: &str, message: String| {
        got.push_str(&format!("== {who}\n   -> {message}\n"));
    };
    said(
        "sweep (Credentials::from_env)",
        crate::cloudflare::rest::for_purpose("sweep", crate::migrate::SWEEP_TOKEN_NEEDS)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default(),
    );
    said(
        "Hyperdrive",
        super::hyperdrive::RestHyperdrive::from_env()
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default(),
    );
    said(
        "email destinations",
        // The fixture pinned the old `LookupError`'s debug form around the message; the message
        // inside it is what a user reads.
        format!(
            "{:?}",
            super::destinations::RestDestinations::from_env()
                .err()
                .map(|e| Failed(e.to_string()))
        ),
    );
    said(
        "account listings",
        super::on_account::RestListings::from_env()
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default(),
    );
    said(
        "email senders",
        super::senders::RestSendingDomains::from_env()
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default(),
    );
    said(
        "Secrets Store",
        super::linked::RestStores::from_env()
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default(),
    );
    said(
        "AI Search",
        super::linked::RestAiSearch::from_env()
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default(),
    );
    said(
        "mTLS certificates",
        super::linked::RestMtls::from_env()
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default(),
    );
    // 1D.4 (#4056 Decision E): the deploy's pre-flight for a REST-managed kind, and the plan's
    // note when KV's drift read is skipped.
    let kv = crate::cloudflare::rest::for_kind(crate::cloudflare::rest::Purpose::Kv).err();
    said(
        "KV namespaces (pre-flight)",
        kv.as_ref().map(|e| e.to_string()).unwrap_or_default(),
    );
    for (who, purpose) in [
        (
            "R2 buckets (pre-flight)",
            crate::cloudflare::rest::Purpose::R2,
        ),
        (
            "D1 databases (pre-flight)",
            crate::cloudflare::rest::Purpose::D1,
        ),
        (
            "Vectorize indexes (pre-flight)",
            crate::cloudflare::rest::Purpose::Vectorize,
        ),
        (
            "Queues (pre-flight)",
            crate::cloudflare::rest::Purpose::Queues,
        ),
    ] {
        said(
            who,
            crate::cloudflare::rest::for_kind(purpose)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default(),
        );
    }
    said(
        "KV drift read (plan note)",
        kv.as_ref()
            .map(super::plan::kv_unchecked_note)
            .unwrap_or_default(),
    );
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/deploy/fixtures/cloudflare_credentials.txt");
    if std::env::var_os("BYNK_WRITE_MESSAGE_FIXTURE").is_some() {
        std::fs::write(&path, &got).unwrap();
    }
    let want = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        got == want,
        "the credential messages changed. This is a stop-and-report, never a bless (#4043, Decision F).\n--- got ---\n{got}"
    );
}
