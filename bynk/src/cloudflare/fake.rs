//! [`Recording`]: a [`CloudflareApi`] for tests, which logs each call as its wrangler arguments
//! and answers it from the test's own replies (#4043, Decision C).
//!
//! The log line is [`Op::line`], exactly what the on-disk stand-ins' `echo "$*"` wrote, so a call
//! log asserted against a stand-in reads the same against this.

use std::cell::RefCell;
use std::path::Path;

use super::api::{CloudflareApi, Op, PushCall, PushRun, VectorizeOp};
use super::error::{Exit, Failure, WranglerError};

/// What the fake answers a call with: what the operation returns on success (an id, or the
/// printed JSON; ignored by the methods that return `()`), or its error.
pub(crate) type Reply = Result<String, WranglerError>;

type Answer = Box<dyn FnMut(&Op<'_>) -> Reply>;

pub(crate) struct Recording {
    calls: RefCell<Vec<String>>,
    answer: RefCell<Answer>,
}

impl Recording {
    /// A fake that answers every call with `answer`.
    pub(crate) fn new(answer: impl FnMut(&Op<'_>) -> Reply + 'static) -> Self {
        Recording {
            calls: RefCell::new(Vec::new()),
            answer: RefCell::new(Box::new(answer)),
        }
    }

    /// Every call so far, one line each, in order.
    pub(crate) fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }

    fn call(&self, op: Op<'_>) -> Reply {
        self.calls.borrow_mut().push(op.line());
        (self.answer.borrow_mut())(&op)
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

    fn kv_create(&self, title: &str) -> Result<String, WranglerError> {
        self.call(Op::KvCreate { title })
    }

    fn kv_list(&self) -> Result<String, WranglerError> {
        self.call(Op::KvList)
    }

    fn kv_delete(&self, id: &str) -> Result<(), WranglerError> {
        self.call(Op::KvDelete { id }).map(drop)
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

    fn d1_create(&self, name: &str) -> Result<String, WranglerError> {
        self.call(Op::D1Create { name })
    }

    fn d1_list(&self) -> Result<String, WranglerError> {
        self.call(Op::D1List)
    }

    fn r2_create(&self, name: &str, jurisdiction: Option<&str>) -> Result<(), WranglerError> {
        self.call(Op::R2Create { name, jurisdiction }).map(drop)
    }

    fn vectorize(&self, op: VectorizeOp<'_>) -> Result<String, WranglerError> {
        self.call(Op::Vectorize(op))
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

    /// The value is logged as `stdin: <value>` after the call, as the stand-ins logged it.
    fn secret_put(
        &self,
        _worker_dir: &Path,
        name: &str,
        value: &str,
        environment: &str,
    ) -> Result<(), WranglerError> {
        let reply = self.call(Op::SecretPut { name, environment });
        self.calls.borrow_mut().push(format!("stdin: {value}"));
        reply.map(drop)
    }

    fn named_secret(
        &self,
        _dir: &Path,
        verb: &str,
        script: &str,
        name: &str,
        stdin: &str,
    ) -> Result<(), WranglerError> {
        let reply = self.call(Op::NamedSecret { verb, script, name });
        self.calls.borrow_mut().push(format!("stdin: {stdin}"));
        reply.map(drop)
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
