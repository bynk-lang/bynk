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
    /// Whether the process succeeded.
    pub(crate) fn success(self) -> bool {
        self == Exit::Code(0)
    }

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

/// Which wrangler call failed, as far as its error's predicates need to know: one variant per
/// operation that still runs as a wrangler process. A kind that moves to REST (1D.4, #4056)
/// takes its variants with it, so the `wrangler_resource_ops` probe reads what is left here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpKind {
    QueueCreate,
    QueueInfo,
    QueueDelete,
    D1MigrationsApply,
    Push,
    TriggersDeploy,
    Whoami,
    WorkerDeployments,
    SecretList,
    SecretPut,
    NamedSecret,
    Rollback,
    D1MigrationsApplyLocal,
    SecretsStoreListLocal,
}

#[derive(Debug)]
pub(crate) enum Failure {
    /// There is no wrangler to run.
    Missing,
    /// Wrangler could not be started.
    Start(std::io::Error),
    /// Wrangler started, but bynk could not wait for it to finish.
    Wait(std::io::Error),
    /// Something around the call failed: its stdin pipe, a write to it, or its directory.
    Io(std::io::Error),
    /// The call's stdin could not be opened; the text says which.
    Pipe(&'static str),
    /// Wrangler ran and failed. `status` is how the process's own status prints
    /// (`exit status: 1`), for the messages that quote it.
    Exit {
        exit: Exit,
        status: String,
        stdout: String,
        stderr: String,
    },
}

/// A wrangler call that failed.
#[derive(Debug)]
pub(crate) struct WranglerError {
    /// `None` only for a call that runs over REST, which never builds one (a test's misuse).
    op: Option<OpKind>,
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

    /// A failed exit with what wrangler printed. Its status prints as a Unix shell's would.
    pub(crate) fn exit(op: &Op<'_>, exit: Exit, stdout: &str, stderr: &str) -> Self {
        let status = match exit {
            Exit::Code(code) => format!("exit status: {code}"),
            Exit::Signal(signal) => format!("signal: {signal}"),
        };
        Self::exited_with(op, exit, status, stdout, stderr)
    }

    /// A failed exit of a real process, its status printed as the platform prints it.
    pub(crate) fn exited(
        op: &Op<'_>,
        status: &std::process::ExitStatus,
        stdout: &str,
        stderr: &str,
    ) -> Self {
        Self::exited_with(op, Exit::of(status), status.to_string(), stdout, stderr)
    }

    fn exited_with(op: &Op<'_>, exit: Exit, status: String, stdout: &str, stderr: &str) -> Self {
        Self::new(
            op,
            Failure::Exit {
                exit,
                status,
                stdout: stdout.to_string(),
                stderr: stderr.to_string(),
            },
        )
    }

    pub(crate) fn failure(&self) -> &Failure {
        &self.failure
    }

    /// Whether there was no wrangler to run.
    pub(crate) fn is_missing(&self) -> bool {
        matches!(self.failure, Failure::Missing)
    }

    /// What a failed exit wrote to stderr alone, trimmed (`rollback`'s rendering); any other
    /// failure's own text.
    pub(crate) fn stderr_only(&self) -> String {
        match &self.failure {
            Failure::Exit { stderr, .. } => stderr.trim().to_string(),
            _ => self.said(),
        }
    }

    /// [`said`](Self::said), except that a failed exit with nothing on either stream says
    /// `silent` (the migration sweep's `wrangler secret {verb} failed`).
    pub(crate) fn said_or(&self, silent: &str) -> String {
        match &self.failure {
            Failure::Exit { stdout, stderr, .. }
                if stdout.trim().is_empty() && stderr.trim().is_empty() =>
            {
                silent.to_string()
            }
            _ => self.said(),
        }
    }

    /// What a failed exit wrote to stderr, or to stdout when stderr is blank, trimmed (`bynk
    /// dev`'s rendering); any other failure's own text.
    pub(crate) fn stderr_else_stdout(&self) -> String {
        match &self.failure {
            Failure::Exit { stdout, stderr, .. } => {
                if stderr.trim().is_empty() {
                    stdout.trim().to_string()
                } else {
                    stderr.trim().to_string()
                }
            }
            _ => self.said(),
        }
    }

    /// Whether a Worker lookup failed because the Worker is not there: Cloudflare's code 10007,
    /// or 10090 for a legacy environment, as wrangler reads them (`isWorkerNotFoundError`). An
    /// auth or network failure is never mistaken for one.
    pub(crate) fn worker_not_found(&self) -> bool {
        self.exit_said()
            .is_some_and(|said| said.contains("[code: 10007]") || said.contains("[code: 10090]"))
    }

    /// Everything a failed call said, both streams, never empty: a non-zero exit with nothing to
    /// say must still read as something rather than as a dangling colon. Before 1D.3,
    /// `provisioning::wrangler_said`.
    pub(crate) fn said(&self) -> String {
        match &self.failure {
            Failure::Missing => "wrangler not found".to_string(),
            Failure::Start(e) | Failure::Wait(e) | Failure::Io(e) => e.to_string(),
            Failure::Pipe(what) => what.to_string(),
            Failure::Exit {
                exit,
                stdout,
                stderr,
                ..
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
    pub(crate) fn ran_and_failed(&self) -> bool {
        matches!(self.failure, Failure::Exit { .. })
    }

    /// What a failed exit said, for the prose matches; `None` for any other failure,
    /// which no predicate reads as benign.
    fn exit_said(&self) -> Option<String> {
        self.ran_and_failed().then(|| self.said())
    }

    /// Whether this create failed only because what it creates is already there.
    ///
    /// - A queue: "already exists" on either stream, wrangler being inconsistent about which. The
    ///   race-loser's path (ADR 0194 D2): a queue that was simply there was found by `queues
    ///   info` first.
    pub(crate) fn already_exists(&self) -> bool {
        let Some(said) = self.exit_said() else {
            return false;
        };
        let lower = said.to_ascii_lowercase();
        match self.op {
            Some(OpKind::QueueCreate) => lower.contains("already exists"),
            _ => false,
        }
    }

    /// Whether this delete failed only because what it deletes is already gone, so a re-run
    /// `--prune` is safe (#839).
    ///
    /// - A queue: "does not exist" (`Queue "<name>" does not exist. To create it, …`).
    ///
    /// (A KV namespace's delete runs over REST since 1D.4a, read by its code, 10013.)
    pub(crate) fn already_gone(&self) -> bool {
        let Some(said) = self.exit_said() else {
            return false;
        };
        match self.op {
            Some(OpKind::QueueDelete) => said.to_ascii_lowercase().contains("does not exist"),
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
    fn a_push_lost_its_upload_only_on_wranglers_10007() {
        assert!(push_lost_its_upload(
            "Uploaded x (0.86 sec)\n✘ [ERROR] This Worker does not exist on your account. [code: 10007]"
        ));
        assert!(!push_lost_its_upload(
            "references Worker 'y' which was not found [code: 10143]"
        ));
        assert!(!push_lost_its_upload("[code: 10090]"));
        assert!(!push_lost_its_upload("10007 without the bracket form"));
    }

    #[test]
    fn a_failure_says_both_streams_trimmed_or_its_exit() {
        let op = Op::Whoami;
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
        // Only a failed exit is ever benign, and only for a create.
        assert!(!WranglerError::missing(&queue).already_exists());
        assert!(!exit(&Op::Whoami, 1, "already exists", "").already_exists());
    }

    #[test]
    fn a_refusal_is_a_4xx_that_is_neither_denied_nor_rate_limited() {
        use super::super::rest::RestError;
        let answered = |status, errors: &str| -> CloudflareError {
            RestError::answered("POST", "/r2/buckets", status, errors).into()
        };
        assert!(answered(409, r#"[{"code":10073}]"#).is_refusal());
        assert!(answered(400, r#"[{"code":7999}]"#).is_refusal());
        assert!(!answered(403, r#"[{"code":10000}]"#).is_refusal());
        assert!(
            !answered(400, r#"[{"code":10000}]"#).is_refusal(),
            "an auth code is denied"
        );
        assert!(!answered(429, "[]").is_refusal());
        assert!(!answered(500, "[]").is_refusal());
        assert!(!CloudflareError::Other("x".into()).is_refusal());
        assert!(!CloudflareError::from(WranglerError::missing(&Op::Whoami)).is_refusal());
    }

    #[test]
    fn already_gone_is_read_per_operation() {
        let queue = Op::QueueDelete { name: "q" };
        assert!(exit(&queue, 1, "", "Queue \"q\" does not exist.").already_gone());
        assert!(!exit(&queue, 1, "", "Authentication error [code: 10000]").already_gone());
        assert!(!WranglerError::missing(&queue).already_gone());
        // Only a delete is ever already gone.
        assert!(!exit(&Op::Whoami, 1, "", "does not exist").already_gone());
    }
}

/// Whether a failed push's teed output says the Worker it just uploaded does not exist: wrangler's
/// lookup after the upload racing the script's creation (#3562, code 10007 only; a legacy
/// environment's 10090 is not this race). The push's output is the text it streamed, not a
/// [`WranglerError`], so this reads that text; it lives here so that every match on wrangler's
/// words is in this module (1D.3c). Before 1D.3c, `first_deploy::NOT_FOUND`.
pub(crate) fn push_lost_its_upload(said: &str) -> bool {
    said.contains("[code: 10007]")
}

/// Any failure of a call to Cloudflare (#4043, Decision E): a wrangler call, a REST call, or a
/// missing credential. Its `Display` is the text bynk printed before 1D.3. Typed errors stop at
/// the client boundary: above it, each caller's own `format!` and `DeployFailure` are unchanged.
#[derive(Debug)]
pub(crate) enum CloudflareError {
    Wrangler(WranglerError),
    Rest(super::rest::RestError),
    /// A missing credential, in the words of the purpose that needed it.
    Credentials(String),
    /// A failure stated in words: an answer that succeeded but could not be used (`the account
    /// has no workers.dev subdomain`), or the TLS roots that could not be loaded.
    Other(String),
}

impl CloudflareError {
    /// The REST failure, when it is one.
    pub(crate) fn rest(&self) -> Option<&super::rest::RestError> {
        match self {
            CloudflareError::Rest(e) => Some(e),
            _ => None,
        }
    }

    /// The status Cloudflare answered with, when a REST call was answered.
    pub(crate) fn status(&self) -> Option<u16> {
        self.rest().and_then(super::rest::RestError::status)
    }

    /// Whether the token lacks the permission ([`super::rest::RestError::is_denied`]).
    pub(crate) fn is_denied(&self) -> bool {
        self.rest().is_some_and(super::rest::RestError::is_denied)
    }

    /// Whether Cloudflare answered and refused the request itself: a 4xx that is not a denied
    /// token (401, 403 or an authentication code) or a rate limit (429). A create refused this way
    /// may have found what it creates already there, which only reading the account can tell
    /// (1D.4b, #4056): the reference documents no "already exists" code for D1, and none for R2
    /// that tells this account's bucket from another's.
    pub(crate) fn is_refusal(&self) -> bool {
        self.status()
            .is_some_and(|status| (400..500).contains(&status) && status != 429)
            && !self.is_denied()
    }
}

impl fmt::Display for CloudflareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CloudflareError::Wrangler(e) => e.fmt(f),
            CloudflareError::Rest(e) => e.fmt(f),
            CloudflareError::Credentials(s) | CloudflareError::Other(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for CloudflareError {}

/// Two failures are the same when they print the same: the text is the contract (Decision F),
/// and what a state like `ScriptState::Unknown` is compared by. (A `Failure` holds an
/// `io::Error`, which has no equality of its own.)
impl PartialEq for CloudflareError {
    fn eq(&self, other: &Self) -> bool {
        self.to_string() == other.to_string()
    }
}

impl Eq for CloudflareError {}

impl From<WranglerError> for CloudflareError {
    fn from(e: WranglerError) -> Self {
        CloudflareError::Wrangler(e)
    }
}

impl From<super::rest::RestError> for CloudflareError {
    fn from(e: super::rest::RestError) -> Self {
        CloudflareError::Rest(e)
    }
}

impl From<String> for CloudflareError {
    fn from(s: String) -> Self {
        CloudflareError::Other(s)
    }
}

/// Above the client boundary a caller's own error is still its text (`DeployFailure`, a
/// revert's `String`): `?` carries a typed error up as exactly what it prints.
impl From<CloudflareError> for String {
    fn from(e: CloudflareError) -> Self {
        e.to_string()
    }
}

impl From<&str> for CloudflareError {
    fn from(s: &str) -> Self {
        CloudflareError::Other(s.to_string())
    }
}
