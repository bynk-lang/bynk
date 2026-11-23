//! [`Recording`]: a [`CloudflareApi`] for tests, which logs each call as its wrangler arguments
//! and answers it from the test's own replies (#4043, Decision C).
//!
//! The log line is [`Op::line`], exactly what the on-disk stand-ins' `echo "$*"` wrote, so a call
//! log asserted against a stand-in reads the same against this.

use std::cell::RefCell;
use std::path::Path;

use super::api::{CloudflareApi, Op, VectorizeOp};
use super::error::{Exit, WranglerError};

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
}
