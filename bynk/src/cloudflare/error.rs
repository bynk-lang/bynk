//! What a failed wrangler call says (#4043, Decision E).
//!
//! A [`WranglerError`]'s `Display` is the text bynk printed before 1D.3: both streams, trimmed
//! and joined, or `wrangler exited N` when wrangler said nothing, and `wrangler not found` when
//! there is no wrangler. The predicates that read wrangler's prose to tell a benign failure from
//! a real one ("already exists", "already gone") live here, with their exact substrings, so 1D.4
//! can empty this file kind by kind as each operation moves to REST.

use std::fmt;

use super::api::Op;

/// How a process ended: an exit code, or the signal that stopped it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Exit {
    Code(i32),
    Signal(i32),
}

impl Exit {
    /// The process's exit as one byte, as a shell reports it: the code (clamped), 128 plus the
    /// signal, or 0 for SIGINT, which a shared Ctrl-C delivers to bynk too. The same rule as
    /// [`crate::shell::exit_status_byte`].
    pub(crate) fn byte(self) -> u8 {
        match self {
            Exit::Code(code) => code.clamp(0, 255) as u8,
            Exit::Signal(2) => 0,
            Exit::Signal(signal) => 128u8.saturating_add(signal.clamp(0, 127) as u8),
        }
    }

    pub(crate) fn of(status: &std::process::ExitStatus) -> Exit {
        if let Some(code) = status.code() {
            return Exit::Code(code);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            if let Some(signal) = status.signal() {
                return Exit::Signal(signal);
            }
        }
        Exit::Code(1)
    }
}

/// Which call failed, as far as its error's predicates need to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpKind {
    QueueCreate,
    QueueInfo,
    QueueDelete,
    KvCreate,
    KvList,
    KvDelete,
    D1MigrationsApply,
    D1Create,
    D1List,
    R2Create,
    VectorizeList,
    VectorizeGet,
    VectorizeCreate,
    VectorizeListMetadataIndex,
    VectorizeCreateMetadataIndex,
    VectorizeDelete,
}

#[derive(Debug)]
pub(crate) enum Failure {
    /// There is no wrangler to run.
    Missing,
    /// Wrangler could not be started.
    Start(std::io::Error),
    /// Wrangler ran and failed.
    Exit {
        exit: Exit,
        stdout: String,
        stderr: String,
    },
    /// Wrangler succeeded, but what it printed could not be read; the text says what was missing.
    Unreadable(String),
}

/// A wrangler call that failed.
#[derive(Debug)]
pub(crate) struct WranglerError {
    op: OpKind,
    failure: Failure,
}

impl WranglerError {
    pub(crate) fn new(op: &Op<'_>, failure: Failure) -> Self {
        WranglerError {
            op: op.kind(),
            failure,
        }
    }

    pub(crate) fn missing(op: &Op<'_>) -> Self {
        Self::new(op, Failure::Missing)
    }

    /// A failed exit with what wrangler printed.
    pub(crate) fn exit(op: &Op<'_>, exit: Exit, stdout: &str, stderr: &str) -> Self {
        Self::new(
            op,
            Failure::Exit {
                exit,
                stdout: stdout.to_string(),
                stderr: stderr.to_string(),
            },
        )
    }

    /// Everything a failed call said, both streams, never empty: a non-zero exit with nothing to
    /// say must still read as something rather than as a dangling colon. Before 1D.3,
    /// `provisioning::wrangler_said`.
    pub(crate) fn said(&self) -> String {
        match &self.failure {
            Failure::Missing => "wrangler not found".to_string(),
            Failure::Start(e) => e.to_string(),
            Failure::Unreadable(what) => what.clone(),
            Failure::Exit {
                exit,
                stdout,
                stderr,
            } => {
                let said: Vec<&str> = [stderr.trim(), stdout.trim()]
                    .into_iter()
                    .filter(|stream| !stream.is_empty())
                    .collect();
                if said.is_empty() {
                    format!("wrangler exited {}", exit.byte())
                } else {
                    said.join("\n")
                }
            }
        }
    }

    /// Whether wrangler ran and failed, rather than not running at all or answering something
    /// unreadable.
    pub(crate) fn exited(&self) -> bool {
        matches!(self.failure, Failure::Exit { .. })
    }

    /// What a failed exit said, for the prose matches; `None` for any other failure,
    /// which no predicate reads as benign.
    fn exit_said(&self) -> Option<String> {
        self.exited().then(|| self.said())
    }

    /// Whether this create failed only because what it creates is already there.
    ///
    /// - A queue: "already exists" on either stream, wrangler being inconsistent about which. The
    ///   race-loser's path (ADR 0194 D2): a queue that was simply there was found by `queues
    ///   info` first.
    /// - An R2 bucket: "already exists" and "you own it" (code 10004). A bucket another account
    ///   owns says it is "not available" and stays a failure.
    /// - A D1 database: "database with that name already exists", which the kind follows with a
    ///   `d1 list` to adopt it (P4a, #2602).
    /// - A metadata index: "metadata index already exists", as written: listed between the
    ///   listing and the create, since Cloudflare builds them asynchronously.
    pub(crate) fn already_exists(&self) -> bool {
        let Some(said) = self.exit_said() else {
            return false;
        };
        let lower = said.to_ascii_lowercase();
        match self.op {
            OpKind::QueueCreate => lower.contains("already exists"),
            OpKind::R2Create => lower.contains("already exists") && lower.contains("you own it"),
            OpKind::D1Create => lower.contains("database with that name already exists"),
            OpKind::VectorizeCreateMetadataIndex => said.contains("metadata index already exists"),
            _ => false,
        }
    }

    /// Whether this delete failed only because what it deletes is already gone, so a re-run
    /// `--prune` is safe (#839).
    ///
    /// - A KV namespace: `code: 10013`, or "namespace not found" (`namespace not found [code:
    ///   10013]`, confirmed against a real account). The code is the more durable half.
    /// - A queue: "does not exist" (`Queue "<name>" does not exist. To create it, …`).
    pub(crate) fn already_gone(&self) -> bool {
        let Some(said) = self.exit_said() else {
            return false;
        };
        let lower = said.to_ascii_lowercase();
        match self.op {
            OpKind::KvDelete => {
                said.contains("code: 10013") || lower.contains("namespace not found")
            }
            OpKind::QueueDelete => lower.contains("does not exist"),
            _ => false,
        }
    }
}

impl fmt::Display for WranglerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.said())
    }
}

impl std::error::Error for WranglerError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn exit(op: &Op<'_>, code: i32, stdout: &str, stderr: &str) -> WranglerError {
        WranglerError::exit(op, Exit::Code(code), stdout, stderr)
    }

    #[test]
    fn a_failure_says_both_streams_trimmed_or_its_exit() {
        let op = Op::KvList;
        assert_eq!(exit(&op, 1, " out \n", "\n err ").said(), "err\nout");
        assert_eq!(exit(&op, 1, "", "err").said(), "err");
        assert_eq!(exit(&op, 3, "", "").said(), "wrangler exited 3");
        assert_eq!(
            WranglerError::exit(&op, Exit::Signal(15), "", "").said(),
            "wrangler exited 143"
        );
        assert_eq!(
            WranglerError::exit(&op, Exit::Signal(2), "", "").said(),
            "wrangler exited 0"
        );
        assert_eq!(
            WranglerError::missing(&op).to_string(),
            "wrangler not found"
        );
    }

    #[test]
    fn the_exit_byte_follows_the_shell() {
        assert_eq!(Exit::Code(300).byte(), 255);
        assert_eq!(Exit::Code(-1).byte(), 0);
        assert_eq!(Exit::Signal(9).byte(), 137);
        assert_eq!(Exit::Signal(2).byte(), 0);
    }

    #[test]
    fn already_exists_is_read_per_operation() {
        let queue = Op::QueueCreate { name: "q" };
        assert!(exit(&queue, 1, "Queue Already Exists", "").already_exists());
        let r2 = Op::R2Create {
            name: "b",
            jurisdiction: None,
        };
        assert!(exit(&r2, 1, "", "already exists, and you own it").already_exists());
        assert!(!exit(&r2, 1, "", "already exists and is not available").already_exists());
        let d1 = Op::D1Create { name: "d" };
        assert!(exit(&d1, 1, "", "A database with that name already exists").already_exists());
        assert!(!exit(&d1, 1, "", "already exists").already_exists());
        let meta = Op::Vectorize(VectorizeOp::CreateMetadataIndex {
            name: "i",
            property: "p",
            ty: "string",
        });
        assert!(exit(&meta, 1, "", "metadata index already exists [code: 40004]").already_exists());
        assert!(!exit(&meta, 1, "", "Metadata Index Already Exists").already_exists());
        // Only a failed exit is ever benign, and only for a create.
        assert!(!WranglerError::missing(&queue).already_exists());
        assert!(!exit(&Op::KvList, 1, "already exists", "").already_exists());
    }

    #[test]
    fn already_gone_is_read_per_operation() {
        let kv = Op::KvDelete { id: "i" };
        assert!(exit(&kv, 1, "", "namespace not found [code: 10013]").already_gone());
        assert!(exit(&kv, 1, "", "code: 10013").already_gone());
        assert!(exit(&kv, 1, "", "Namespace Not Found").already_gone());
        assert!(!exit(&kv, 1, "", "Authentication error [code: 10000]").already_gone());
        let queue = Op::QueueDelete { name: "q" };
        assert!(exit(&queue, 1, "", "Queue \"q\" does not exist.").already_gone());
        assert!(!WranglerError::missing(&kv).already_gone());
    }

    use super::super::api::VectorizeOp;
}
