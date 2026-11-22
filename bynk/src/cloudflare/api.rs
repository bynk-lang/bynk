//! The account operations, one method each (#4043, Decision A), and [`Op`], which names a call
//! and renders the arguments wrangler is run with (Decision C).

use std::path::Path;

use super::error::{OpKind, WranglerError};

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
    /// `d1 create <name>`: the new database's id.
    fn d1_create(&self, name: &str) -> Result<String, WranglerError>;
    /// `d1 list --json`: what it printed.
    fn d1_list(&self) -> Result<String, WranglerError>;
    /// `r2 bucket create <name> [--jurisdiction <j>]`.
    fn r2_create(&self, name: &str, jurisdiction: Option<&str>) -> Result<(), WranglerError>;
    /// One of the six Vectorize calls: what it printed.
    fn vectorize(&self, op: VectorizeOp<'_>) -> Result<String, WranglerError>;
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
    },
    D1List,
    R2Create {
        name: &'a str,
        jurisdiction: Option<&'a str>,
    },
    Vectorize(VectorizeOp<'a>),
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
            Op::D1Create { name } => words(&["d1", "create", name]),
            Op::D1List => words(&["d1", "list", "--json"]),
            Op::R2Create { name, jurisdiction } => {
                let mut argv = words(&["r2", "bucket", "create", name]);
                if let Some(jurisdiction) = jurisdiction {
                    argv.extend(words(&["--jurisdiction", jurisdiction]));
                }
                argv
            }
            Op::Vectorize(op) => {
                let mut argv = words(&["vectorize"]);
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
            (Op::D1Create { name: "db" }, "d1 create db"),
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
    }
}
