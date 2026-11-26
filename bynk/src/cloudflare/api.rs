//! The account operations, one method each (#4043, Decision A), and [`Op`], which names a call
//! and renders the arguments wrangler is run with (Decision C).

use std::path::Path;

use super::error::{Exit, OpKind, WranglerError};

/// Every account operation the resource kinds drive. Today each is a wrangler call
/// ([`super::Wrangler`]); 1D.4 moves them to REST one kind at a time, behind these same methods.
///
/// Each method returns what its caller reads: an id, `()`, or the JSON wrangler printed, which
/// the caller parses as before. The policy stays with the caller: "already exists means adopt"
/// and "already gone means success" are decisions a kind makes over the typed error
/// ([`WranglerError::already_exists`], [`WranglerError::already_gone`]), not the transport's.
pub(crate) trait CloudflareApi {
    /// `queues create <name>`.
    fn queue_create(&self, name: &str) -> Result<(), WranglerError>;
    /// `queues info <name>`: `Ok` when the account has it. Answered by the exit code alone.
    fn queue_info(&self, name: &str) -> Result<(), WranglerError>;
    /// `queues delete <name>`.
    fn queue_delete(&self, name: &str) -> Result<(), WranglerError>;
    /// `kv namespace create <title>`: the new namespace's id.
    fn kv_create(&self, title: &str) -> Result<String, WranglerError>;
    /// `kv namespace list`: its JSON array.
    fn kv_list(&self) -> Result<String, WranglerError>;
    /// `kv namespace delete --namespace-id <id> --skip-confirmation`.
    fn kv_delete(&self, id: &str) -> Result<(), WranglerError>;
    /// `d1 migrations apply <binding> --remote [--env <environment>]`, from `worker_dir`, with
    /// stdin closed so wrangler takes its "yes" fallback.
    fn d1_migrations_apply(
        &self,
        worker_dir: &Path,
        binding: &str,
        environment: &str,
    ) -> Result<(), WranglerError>;
    /// `d1 create <name> [--jurisdiction <j>]`: the new database's id.
    fn d1_create(&self, name: &str, jurisdiction: Option<&str>) -> Result<String, WranglerError>;
    /// `d1 list --json`: what it printed.
    fn d1_list(&self) -> Result<String, WranglerError>;
    /// `r2 bucket create <name> [--jurisdiction <j>]`.
    fn r2_create(&self, name: &str, jurisdiction: Option<&str>) -> Result<(), WranglerError>;
    /// One of the six Vectorize calls: what it printed.
    fn vectorize(&self, op: VectorizeOp<'_>) -> Result<String, WranglerError>;

    // The Worker (1D.3b).

    /// `deploy [args…] [--secrets-file <f>] [--env <e>]`, from the Worker's directory: how it
    /// ended, and what it wrote to stderr when that was teed. The process control is the
    /// adapter's: stderr is copied through and kept when `tee` asks, and with a secrets file
    /// SIGINT and SIGTERM are held until wrangler exits, a SIGTERM being passed on to it, so the
    /// caller can always remove the file. The caller decides what the exit means.
    fn push(&self, call: PushCall<'_>) -> Result<PushRun, WranglerError>;
    /// `triggers deploy [--env <e>]`, from the Worker's directory.
    fn triggers_deploy(&self, worker_dir: &Path, environment: &str) -> Result<(), WranglerError>;
    /// `whoami`: `Ok` when wrangler is authenticated. Answered by the exit code alone.
    fn whoami(&self) -> Result<(), WranglerError>;
    /// `deployments list --name <name> --json`: `Ok` when the account has the Worker; the
    /// caller reads a missing one from the error ([`WranglerError::worker_not_found`]).
    fn worker_deployments(&self, name: &str) -> Result<(), WranglerError>;
    /// `rollback <version> --name <script> --message "bynk revert" --yes`.
    fn rollback(&self, script: &str, version: &str) -> Result<(), WranglerError>;

    // Secrets.

    /// `secret list --format json [--env <e>]`, from the Worker's directory: what it printed.
    fn secret_list(&self, worker_dir: &Path, environment: &str) -> Result<String, WranglerError>;
    /// `secret put <name> [--env <e>]`, from the Worker's directory, the value on stdin and never
    /// in argv (ADR 0195 D1).
    fn secret_put(
        &self,
        worker_dir: &Path,
        name: &str,
        value: &str,
        environment: &str,
    ) -> Result<(), WranglerError>;
    /// `secret <verb> <name> --name <script>` against a deployed Worker by name, run in `dir`
    /// (an empty directory, so no project config is read), `stdin` on stdin.
    fn named_secret(
        &self,
        dir: &Path,
        verb: &str,
        script: &str,
        name: &str,
        stdin: &str,
    ) -> Result<(), WranglerError>;

    // `bynk dev`'s local state.

    /// `d1 migrations apply <binding> --local [--persist-to …]`, from the Worker's directory,
    /// stdin closed.
    fn d1_migrations_apply_local(
        &self,
        worker_dir: &Path,
        binding: &str,
        persist: &[String],
    ) -> Result<(), WranglerError>;
    /// `secrets-store secret list <store> [--persist-to …]`, from the Worker's directory, stdin
    /// closed: what it printed.
    fn secrets_store_list_local(
        &self,
        worker_dir: &Path,
        store: &str,
        persist: &[String],
    ) -> Result<String, WranglerError>;
}

/// One `wrangler deploy`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PushCall<'a> {
    pub(crate) worker_dir: &'a Path,
    pub(crate) args: &'a [String],
    /// A first deploy's secrets, uploaded with the Worker (#2055).
    pub(crate) secrets_file: Option<&'a Path>,
    pub(crate) environment: &'a str,
    /// Copy wrangler's stderr through and keep it (#3562's watch of a first deploy).
    pub(crate) tee: bool,
}

/// How a `wrangler deploy` ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PushRun {
    pub(crate) exit: Exit,
    /// What wrangler wrote to stderr, when the call teed it.
    pub(crate) said: String,
    /// A signal `bynk` received while it held them for a secrets-file upload.
    pub(crate) signalled: Option<i32>,
}

/// The six Vectorize calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VectorizeOp<'a> {
    List,
    Get {
        name: &'a str,
    },
    Create {
        name: &'a str,
        dimensions: i64,
        metric: &'a str,
    },
    ListMetadataIndex {
        name: &'a str,
    },
    CreateMetadataIndex {
        name: &'a str,
        property: &'a str,
        ty: &'a str,
    },
    Delete {
        name: &'a str,
    },
}

/// One account call, as its wrangler arguments: what the adapter runs and what the recording fake
/// logs, so a test's call log reads as the stand-ins' `$*` always did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op<'a> {
    QueueCreate {
        name: &'a str,
    },
    QueueInfo {
        name: &'a str,
    },
    QueueDelete {
        name: &'a str,
    },
    KvCreate {
        title: &'a str,
    },
    KvList,
    KvDelete {
        id: &'a str,
    },
    D1MigrationsApply {
        binding: &'a str,
        environment: &'a str,
    },
    D1Create {
        name: &'a str,
        jurisdiction: Option<&'a str>,
    },
    D1List,
    R2Create {
        name: &'a str,
        jurisdiction: Option<&'a str>,
    },
    Vectorize(VectorizeOp<'a>),
    Push {
        args: &'a [String],
        secrets_file: Option<&'a Path>,
        environment: &'a str,
    },
    TriggersDeploy {
        environment: &'a str,
    },
    Whoami,
    WorkerDeployments {
        name: &'a str,
    },
    Rollback {
        script: &'a str,
        version: &'a str,
    },
    SecretList {
        environment: &'a str,
    },
    SecretPut {
        name: &'a str,
        environment: &'a str,
    },
    NamedSecret {
        verb: &'a str,
        script: &'a str,
        name: &'a str,
    },
    D1MigrationsApplyLocal {
        binding: &'a str,
        persist: &'a [String],
    },
    SecretsStoreListLocal {
        store: &'a str,
        persist: &'a [String],
    },
}

/// `--env <environment>` for a named environment; nothing for the default one.
fn env_args(environment: &str) -> Vec<String> {
    if environment == "default" {
        Vec::new()
    } else {
        vec!["--env".to_string(), environment.to_string()]
    }
}

impl Op<'_> {
    /// The arguments wrangler is run with, its subcommand first.
    pub(crate) fn argv(&self) -> Vec<String> {
        let words = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        match *self {
            Op::QueueCreate { name } => words(&["queues", "create", name]),
            Op::QueueInfo { name } => words(&["queues", "info", name]),
            Op::QueueDelete { name } => words(&["queues", "delete", name]),
            // No `--json`: `kv namespace create` has no such flag (`Unknown argument: json`,
            // #2428). The id is read from the binding snippet it prints instead.
            Op::KvCreate { title } => words(&["kv", "namespace", "create", title]),
            // No `--format json` either: the default output is already a JSON array.
            Op::KvList => words(&["kv", "namespace", "list"]),
            // `--skip-confirmation`: `--prune` already confirmed the whole batch once.
            Op::KvDelete { id } => words(&[
                "kv",
                "namespace",
                "delete",
                "--namespace-id",
                id,
                "--skip-confirmation",
            ]),
            Op::D1MigrationsApply {
                binding,
                environment,
            } => {
                let mut argv = words(&["d1", "migrations", "apply", binding, "--remote"]);
                if environment != "default" {
                    argv.extend(words(&["--env", environment]));
                }
                argv
            }
            Op::D1Create { name, jurisdiction } => {
                let mut argv = words(&["d1", "create", name]);
                if let Some(jurisdiction) = jurisdiction {
                    argv.extend(words(&["--jurisdiction", jurisdiction]));
                }
                argv
            }
            Op::D1List => words(&["d1", "list", "--json"]),
            Op::R2Create { name, jurisdiction } => {
                let mut argv = words(&["r2", "bucket", "create", name]);
                if let Some(jurisdiction) = jurisdiction {
                    argv.extend(words(&["--jurisdiction", jurisdiction]));
                }
                argv
            }
            Op::Vectorize(op) => {
                let mut argv = vec!["vectorize".to_string()];
                argv.extend(match op {
                    VectorizeOp::List => words(&["list", "--json"]),
                    VectorizeOp::Get { name } => words(&["get", name, "--json"]),
                    VectorizeOp::Create {
                        name,
                        dimensions,
                        metric,
                    } => words(&[
                        "create",
                        name,
                        "--dimensions",
                        &dimensions.to_string(),
                        "--metric",
                        metric,
                    ]),
                    VectorizeOp::ListMetadataIndex { name } => {
                        words(&["list-metadata-index", name, "--json"])
                    }
                    VectorizeOp::CreateMetadataIndex { name, property, ty } => words(&[
                        "create-metadata-index",
                        name,
                        "--propertyName",
                        property,
                        "--type",
                        ty,
                    ]),
                    VectorizeOp::Delete { name } => words(&["delete", name, "--force"]),
                });
                argv
            }
            Op::Push {
                args,
                secrets_file,
                environment,
            } => {
                let mut argv = words(&["deploy"]);
                argv.extend(args.iter().cloned());
                if let Some(file) = secrets_file {
                    argv.push("--secrets-file".to_string());
                    argv.push(file.to_string_lossy().into_owned());
                }
                argv.extend(env_args(environment));
                argv
            }
            Op::TriggersDeploy { environment } => {
                let mut argv = words(&["triggers", "deploy"]);
                argv.extend(env_args(environment));
                argv
            }
            Op::Whoami => words(&["whoami"]),
            Op::WorkerDeployments { name } => {
                words(&["deployments", "list", "--name", name, "--json"])
            }
            Op::Rollback { script, version } => words(&[
                "rollback",
                version,
                "--name",
                script,
                "--message",
                "bynk revert",
                "--yes",
            ]),
            Op::SecretList { environment } => {
                let mut argv = words(&["secret", "list", "--format", "json"]);
                argv.extend(env_args(environment));
                argv
            }
            Op::SecretPut { name, environment } => {
                let mut argv = words(&["secret", "put", name]);
                argv.extend(env_args(environment));
                argv
            }
            Op::NamedSecret { verb, script, name } => {
                words(&["secret", verb, name, "--name", script])
            }
            Op::D1MigrationsApplyLocal { binding, persist } => {
                let mut argv = words(&["d1", "migrations", "apply", binding, "--local"]);
                argv.extend(persist.iter().cloned());
                argv
            }
            Op::SecretsStoreListLocal { store, persist } => {
                let mut argv = words(&["secrets-store", "secret", "list", store]);
                argv.extend(persist.iter().cloned());
                argv
            }
        }
    }

    pub(crate) fn kind(&self) -> OpKind {
        match self {
            Op::QueueCreate { .. } => OpKind::QueueCreate,
            Op::QueueInfo { .. } => OpKind::QueueInfo,
            Op::QueueDelete { .. } => OpKind::QueueDelete,
            Op::KvCreate { .. } => OpKind::KvCreate,
            Op::KvList => OpKind::KvList,
            Op::KvDelete { .. } => OpKind::KvDelete,
            Op::D1MigrationsApply { .. } => OpKind::D1MigrationsApply,
            Op::D1Create { .. } => OpKind::D1Create,
            Op::D1List => OpKind::D1List,
            Op::R2Create { .. } => OpKind::R2Create,
            Op::Vectorize(VectorizeOp::List) => OpKind::VectorizeList,
            Op::Vectorize(VectorizeOp::Get { .. }) => OpKind::VectorizeGet,
            Op::Vectorize(VectorizeOp::Create { .. }) => OpKind::VectorizeCreate,
            Op::Vectorize(VectorizeOp::ListMetadataIndex { .. }) => {
                OpKind::VectorizeListMetadataIndex
            }
            Op::Vectorize(VectorizeOp::CreateMetadataIndex { .. }) => {
                OpKind::VectorizeCreateMetadataIndex
            }
            Op::Vectorize(VectorizeOp::Delete { .. }) => OpKind::VectorizeDelete,
            Op::Push { .. } => OpKind::Push,
            Op::TriggersDeploy { .. } => OpKind::TriggersDeploy,
            Op::Whoami => OpKind::Whoami,
            Op::WorkerDeployments { .. } => OpKind::WorkerDeployments,
            Op::Rollback { .. } => OpKind::Rollback,
            Op::SecretList { .. } => OpKind::SecretList,
            Op::SecretPut { .. } => OpKind::SecretPut,
            Op::NamedSecret { .. } => OpKind::NamedSecret,
            Op::D1MigrationsApplyLocal { .. } => OpKind::D1MigrationsApplyLocal,
            Op::SecretsStoreListLocal { .. } => OpKind::SecretsStoreListLocal,
        }
    }

    /// The call as one log line: what a stand-in's `echo "$*"` printed.
    #[cfg(test)]
    pub(crate) fn line(&self) -> String {
        self.argv().join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_operation_renders_the_arguments_it_always_ran_with() {
        let cases: &[(Op<'_>, &str)] = &[
            (Op::QueueCreate { name: "jobs" }, "queues create jobs"),
            (Op::QueueInfo { name: "jobs" }, "queues info jobs"),
            (Op::QueueDelete { name: "jobs" }, "queues delete jobs"),
            (Op::KvCreate { title: "api" }, "kv namespace create api"),
            (Op::KvList, "kv namespace list"),
            (
                Op::KvDelete { id: "0a" },
                "kv namespace delete --namespace-id 0a --skip-confirmation",
            ),
            (
                Op::D1MigrationsApply {
                    binding: "DB",
                    environment: "default",
                },
                "d1 migrations apply DB --remote",
            ),
            (
                Op::D1MigrationsApply {
                    binding: "DB",
                    environment: "staging",
                },
                "d1 migrations apply DB --remote --env staging",
            ),
            (
                Op::D1Create {
                    name: "db",
                    jurisdiction: None,
                },
                "d1 create db",
            ),
            (
                Op::D1Create {
                    name: "db",
                    jurisdiction: Some("eu"),
                },
                "d1 create db --jurisdiction eu",
            ),
            (Op::D1List, "d1 list --json"),
            (
                Op::R2Create {
                    name: "b",
                    jurisdiction: None,
                },
                "r2 bucket create b",
            ),
            (
                Op::R2Create {
                    name: "b",
                    jurisdiction: Some("eu"),
                },
                "r2 bucket create b --jurisdiction eu",
            ),
            (Op::Vectorize(VectorizeOp::List), "vectorize list --json"),
            (
                Op::Vectorize(VectorizeOp::Get { name: "i" }),
                "vectorize get i --json",
            ),
            (
                Op::Vectorize(VectorizeOp::Create {
                    name: "i",
                    dimensions: 768,
                    metric: "cosine",
                }),
                "vectorize create i --dimensions 768 --metric cosine",
            ),
            (
                Op::Vectorize(VectorizeOp::ListMetadataIndex { name: "i" }),
                "vectorize list-metadata-index i --json",
            ),
            (
                Op::Vectorize(VectorizeOp::CreateMetadataIndex {
                    name: "i",
                    property: "p",
                    ty: "string",
                }),
                "vectorize create-metadata-index i --propertyName p --type string",
            ),
            (
                Op::Vectorize(VectorizeOp::Delete { name: "i" }),
                "vectorize delete i --force",
            ),
        ];
        for (op, line) in cases {
            assert_eq!(op.line(), *line);
        }
        let args = ["--var".to_string(), "A:1".to_string()];
        let persist = ["--persist-to".to_string(), "state".to_string()];
        let file = Path::new("/tmp/s/secrets.json");
        let rest: &[(Op<'_>, &str)] = &[
            (
                Op::Push {
                    args: &[],
                    secrets_file: None,
                    environment: "default",
                },
                "deploy",
            ),
            (
                Op::Push {
                    args: &args,
                    secrets_file: Some(file),
                    environment: "staging",
                },
                "deploy --var A:1 --secrets-file /tmp/s/secrets.json --env staging",
            ),
            (
                Op::TriggersDeploy {
                    environment: "staging",
                },
                "triggers deploy --env staging",
            ),
            (Op::Whoami, "whoami"),
            (
                Op::WorkerDeployments { name: "billing" },
                "deployments list --name billing --json",
            ),
            (
                Op::Rollback {
                    script: "s",
                    version: "v",
                },
                "rollback v --name s --message bynk revert --yes",
            ),
            (
                Op::SecretList {
                    environment: "default",
                },
                "secret list --format json",
            ),
            (
                Op::SecretPut {
                    name: "T",
                    environment: "staging",
                },
                "secret put T --env staging",
            ),
            (
                Op::NamedSecret {
                    verb: "delete",
                    script: "s",
                    name: "K",
                },
                "secret delete K --name s",
            ),
            (
                Op::D1MigrationsApplyLocal {
                    binding: "DB",
                    persist: &persist,
                },
                "d1 migrations apply DB --local --persist-to state",
            ),
            (
                Op::SecretsStoreListLocal {
                    store: "st",
                    persist: &[],
                },
                "secrets-store secret list st",
            ),
        ];
        for (op, line) in rest {
            assert_eq!(op.line(), *line);
        }
    }
}
