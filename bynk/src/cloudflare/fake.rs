//! [`Recording`]: a [`CloudflareApi`] for tests, which logs each call as its wrangler arguments
//! and answers it from the test's own replies (#4043, Decision C).
//!
//! The log line is [`Op::line`], exactly what the on-disk stand-ins' `echo "$*"` wrote, so a call
//! log asserted against a stand-in reads the same against this.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::Path;

use super::api::{CloudflareApi, Op, PushCall, PushRun, VectorizeOp};
use super::error::{CloudflareError, Exit, Failure, WranglerError};
use super::rest::vectorize::IndexConfig;

/// What the fake answers a call with: what the operation returns on success (an id, the printed
/// JSON, or for `kv_list` the ids separated by whitespace; ignored by the methods that return
/// `()`), or its error: a [`WranglerError`] for a wrangler operation, a REST failure for a moved
/// one (KV, 1D.4a).
pub(crate) type Reply = Result<String, CloudflareError>;

type Answer = Box<dyn FnMut(&Op<'_>) -> Reply>;

pub(crate) struct Recording {
    calls: RefCell<Vec<String>>,
    /// What each call that takes a value on stdin was sent, in order.
    stdin: RefCell<Vec<String>>,
    answer: RefCell<Answer>,
}

impl Recording {
    /// A fake that answers every call with `answer`, whose error is a [`WranglerError`] when
    /// the test answers only wrangler operations, or any [`CloudflareError`].
    pub(crate) fn new<E: Into<CloudflareError>>(
        mut answer: impl FnMut(&Op<'_>) -> Result<String, E> + 'static,
    ) -> Self {
        Recording {
            calls: RefCell::new(Vec::new()),
            stdin: RefCell::new(Vec::new()),
            answer: RefCell::new(Box::new(move |op: &Op<'_>| answer(op).map_err(Into::into))),
        }
    }

    /// Every call so far, one line each, in order.
    pub(crate) fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }

    /// What the calls that take a value on stdin were sent, in order: never in [`calls`].
    pub(crate) fn stdin(&self) -> Vec<String> {
        self.stdin.borrow().clone()
    }

    /// A REST operation's call: the reply as it is.
    fn rest(&self, op: Op<'_>) -> Reply {
        self.calls.borrow_mut().push(op.line());
        (self.answer.borrow_mut())(&op)
    }

    /// A wrangler operation's call: its reply's error must be wrangler's.
    fn call(&self, op: Op<'_>) -> Result<String, WranglerError> {
        self.rest(op).map_err(|e| match e {
            CloudflareError::Wrangler(e) => e,
            other => panic!("a wrangler operation answered with a non-wrangler failure: {other}"),
        })
    }
}

/// A failed exit for `op` with what it printed: the shape of a stand-in's `exit 1`.
pub(crate) fn failed(op: &Op<'_>, stdout: &str, stderr: &str) -> WranglerError {
    WranglerError::exit(op, Exit::Code(1), stdout, stderr)
}

impl CloudflareApi for Recording {
    fn queue_create(&self, name: &str) -> Result<(), WranglerError> {
        self.call(Op::QueueCreate { name }).map(drop)
    }

    fn queue_info(&self, name: &str) -> Result<(), WranglerError> {
        self.call(Op::QueueInfo { name }).map(drop)
    }

    fn queue_delete(&self, name: &str) -> Result<(), WranglerError> {
        self.call(Op::QueueDelete { name }).map(drop)
    }

    fn kv_create(&self, title: &str) -> Result<String, CloudflareError> {
        self.rest(Op::KvCreate { title })
    }

    fn kv_list(&self) -> Result<BTreeSet<String>, CloudflareError> {
        let ids = self.rest(Op::KvList)?;
        Ok(ids.split_whitespace().map(str::to_string).collect())
    }

    fn kv_delete(&self, id: &str) -> Result<(), CloudflareError> {
        self.rest(Op::KvDelete { id }).map(drop)
    }

    fn d1_migrations_apply(
        &self,
        _worker_dir: &Path,
        binding: &str,
        environment: &str,
    ) -> Result<(), WranglerError> {
        self.call(Op::D1MigrationsApply {
            binding,
            environment,
        })
        .map(drop)
    }

    fn d1_create(&self, name: &str, jurisdiction: Option<&str>) -> Result<String, CloudflareError> {
        self.rest(Op::D1Create { name, jurisdiction })
    }

    /// Logged as the `d1 list --json` it replaced; the reply is the id found, or empty for none.
    fn d1_find(&self, _name: &str) -> Result<Option<String>, CloudflareError> {
        let id = self.rest(Op::D1List)?;
        Ok(Some(id.trim().to_string()).filter(|id| !id.is_empty()))
    }

    fn r2_create(&self, name: &str, jurisdiction: Option<&str>) -> Result<(), CloudflareError> {
        self.rest(Op::R2Create { name, jurisdiction }).map(drop)
    }

    /// The reply `"owned"` is a bucket this account holds; any other `Ok` is one it does not.
    fn r2_owned(&self, name: &str, jurisdiction: Option<&str>) -> Result<bool, CloudflareError> {
        Ok(self.rest(Op::R2Owned { name, jurisdiction })? == "owned")
    }

    fn vectorize_names(&self) -> Result<Vec<String>, CloudflareError> {
        let reply = self.rest(Op::Vectorize(VectorizeOp::List))?;
        super::rest::vectorize::listed(&result(&reply))
            .map(|(names, _)| names)
            .ok_or_else(|| unreadable("the list of index names"))
    }

    fn vectorize_config(&self, name: &str) -> Result<IndexConfig, CloudflareError> {
        let reply = self.rest(Op::Vectorize(VectorizeOp::Get { name }))?;
        super::rest::vectorize::config(&result(&reply))
            .ok_or_else(|| unreadable("the dimensions and metric"))
    }

    fn vectorize_create(
        &self,
        name: &str,
        dimensions: i64,
        metric: &str,
    ) -> Result<(), CloudflareError> {
        self.rest(Op::Vectorize(VectorizeOp::Create {
            name,
            dimensions,
            metric,
        }))
        .map(drop)
    }

    /// A reply that is a bare array is taken as the `metadataIndexes` list.
    fn vectorize_metadata_indexes(&self, name: &str) -> Result<Vec<String>, CloudflareError> {
        let reply = self.rest(Op::Vectorize(VectorizeOp::ListMetadataIndex { name }))?;
        let mut body = result(&reply);
        if body["result"].is_array() {
            body = serde_json::json!({ "result": { "metadataIndexes": body["result"].take() } });
        }
        super::rest::vectorize::metadata(&body)
            .ok_or_else(|| unreadable("the list of metadata indexes"))
    }

    fn vectorize_create_metadata_index(
        &self,
        name: &str,
        property: &str,
        ty: &str,
    ) -> Result<(), CloudflareError> {
        self.rest(Op::Vectorize(VectorizeOp::CreateMetadataIndex {
            name,
            property,
            ty,
        }))
        .map(drop)
    }

    fn vectorize_delete(&self, name: &str) -> Result<(), CloudflareError> {
        self.rest(Op::Vectorize(VectorizeOp::Delete { name }))
            .map(drop)
    }

    /// A reply of `Ok` is a push that exited 0; a failed exit is a push that ran and failed,
    /// returned as its exit with its stderr, as the adapter returns it. Any other error (no
    /// wrangler, a start failure) is the error.
    fn push(&self, call: PushCall<'_>) -> Result<PushRun, WranglerError> {
        match self.call(Op::Push {
            args: call.args,
            secrets_file: call.secrets_file,
            environment: call.environment,
        }) {
            Ok(_) => Ok(PushRun {
                exit: Exit::Code(0),
                said: String::new(),
                signalled: None,
            }),
            Err(e) => match e.failure() {
                Failure::Exit { exit, stderr, .. } => Ok(PushRun {
                    exit: *exit,
                    said: if call.tee {
                        stderr.clone()
                    } else {
                        String::new()
                    },
                    signalled: None,
                }),
                _ => Err(e),
            },
        }
    }

    fn triggers_deploy(&self, _worker_dir: &Path, environment: &str) -> Result<(), WranglerError> {
        self.call(Op::TriggersDeploy { environment }).map(drop)
    }

    fn whoami(&self) -> Result<(), WranglerError> {
        self.call(Op::Whoami).map(drop)
    }

    fn worker_deployments(&self, name: &str) -> Result<(), WranglerError> {
        self.call(Op::WorkerDeployments { name }).map(drop)
    }

    fn rollback(&self, script: &str, version: &str) -> Result<(), WranglerError> {
        self.call(Op::Rollback { script, version }).map(drop)
    }

    fn secret_list(&self, _worker_dir: &Path, environment: &str) -> Result<String, WranglerError> {
        self.call(Op::SecretList { environment })
    }

    fn secret_put(
        &self,
        _worker_dir: &Path,
        name: &str,
        value: &str,
        environment: &str,
    ) -> Result<(), WranglerError> {
        self.stdin.borrow_mut().push(value.to_string());
        self.call(Op::SecretPut { name, environment }).map(drop)
    }

    fn named_secret(
        &self,
        _dir: &Path,
        verb: &str,
        script: &str,
        name: &str,
        stdin: &str,
    ) -> Result<(), WranglerError> {
        self.stdin.borrow_mut().push(stdin.to_string());
        self.call(Op::NamedSecret { verb, script, name }).map(drop)
    }

    fn d1_migrations_apply_local(
        &self,
        _worker_dir: &Path,
        binding: &str,
        persist: &[String],
    ) -> Result<(), WranglerError> {
        self.call(Op::D1MigrationsApplyLocal { binding, persist })
            .map(drop)
    }

    fn secrets_store_list_local(
        &self,
        _worker_dir: &Path,
        store: &str,
        persist: &[String],
    ) -> Result<String, WranglerError> {
        self.call(Op::SecretsStoreListLocal { store, persist })
    }
}

/// A reply as the body of a REST answer: the reply's JSON as its `result` (`null` when the
/// reply is not JSON).
fn result(reply: &str) -> serde_json::Value {
    serde_json::json!({ "result": serde_json::from_str::<serde_json::Value>(reply).unwrap_or_default() })
}

/// A fake reply that holds no `what`, as the account renders an unreadable answer.
fn unreadable(what: &str) -> CloudflareError {
    CloudflareError::Other(format!("Cloudflare's answer has no {what}"))
}
