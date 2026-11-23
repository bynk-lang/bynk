//! The retry policy for REST calls (1D.3c, #4043 Decision H).
//!
//! Ruled: the machinery is built and tested, and ships at **one attempt** ([`Policy::RULED`])
//! for as long as the freeze holds, because a retry changes what a failure path prints and how
//! long it takes. Turning it on is the one-line change of `RULED` to [`Policy::BACKOFF`], in its
//! own ruled PR.
//!
//! Only an idempotent call is retried (a `GET`, `PUT` or `DELETE`, or a `POST` the caller marks
//! idempotent, like D1's `SELECT`; never `POST /hyperdrive/configs`), and only on a failure a
//! retry could help: the request not completing, or Cloudflare answering 429 or a 5xx. A
//! `Retry-After` is honoured up to a cap. Wrangler's calls are never retried: its failures are
//! text, so a transient one cannot be told from a real one.

use std::time::Duration;

use super::rest::RestError;

/// Whether a call may be sent again without changing what it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Idempotent {
    Yes,
    No,
}

/// How a REST call is retried.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Policy {
    /// How many times a call is made in all: 1 means no retry.
    pub(crate) attempts: u32,
    /// The wait before the second attempt; each later wait doubles, up to `longest`.
    pub(crate) first: Duration,
    pub(crate) longest: Duration,
    /// The longest `Retry-After` honoured.
    pub(crate) retry_after_cap: Duration,
    /// How the policy waits (a test's records instead).
    pub(crate) sleep: fn(Duration),
}

impl Policy {
    /// What ships during the freeze: one attempt, no retry.
    pub(crate) const RULED: Policy = Policy {
        attempts: 1,
        ..Policy::BACKOFF
    };

    /// The policy for after the freeze: three attempts, 0.5 s then 1 s, `Retry-After` capped at
    /// 10 s.
    pub(crate) const BACKOFF: Policy = Policy {
        attempts: 3,
        first: Duration::from_millis(500),
        longest: Duration::from_secs(1),
        retry_after_cap: Duration::from_secs(10),
        sleep: std::thread::sleep,
    };

    /// Run `call`, retrying it as the policy allows.
    pub(crate) fn run<T>(
        &self,
        idempotent: Idempotent,
        mut call: impl FnMut() -> Result<T, RestError>,
    ) -> Result<T, RestError> {
        let mut wait = self.first;
        let mut attempt = 1;
        loop {
            match call() {
                Err(e)
                    if idempotent == Idempotent::Yes
                        && attempt < self.attempts
                        && e.is_transient() =>
                {
                    let asked = e.retry_after().map(|after| after.min(self.retry_after_cap));
                    (self.sleep)(asked.unwrap_or(wait));
                    wait = (wait * 2).min(self.longest);
                    attempt += 1;
                }
                done => return done,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        static SLEPT: RefCell<Vec<Duration>> = const { RefCell::new(Vec::new()) };
    }

    fn record(d: Duration) {
        SLEPT.with(|s| s.borrow_mut().push(d));
    }

    fn slept() -> Vec<Duration> {
        SLEPT.with(|s| s.borrow_mut().drain(..).collect())
    }

    fn three() -> Policy {
        Policy {
            sleep: record,
            ..Policy::BACKOFF
        }
    }

    /// Answers each call with the next of `answers`, counting the calls.
    fn calls(
        answers: Vec<Result<u32, RestError>>,
    ) -> (
        impl FnMut() -> Result<u32, RestError>,
        std::rc::Rc<RefCell<u32>>,
    ) {
        let made = std::rc::Rc::new(RefCell::new(0));
        let counter = made.clone();
        let mut answers = answers.into_iter();
        (
            move || {
                *counter.borrow_mut() += 1;
                answers.next().expect("an answer per call")
            },
            made,
        )
    }

    fn server(status: u16) -> RestError {
        RestError::answered("GET", "/x", status, "[]")
    }

    #[test]
    fn the_ruled_policy_makes_one_attempt() {
        let (call, made) = calls(vec![Err(server(503))]);
        let policy = Policy {
            sleep: record,
            ..Policy::RULED
        };
        assert_eq!(policy.run(Idempotent::Yes, call), Err(server(503)));
        assert_eq!(*made.borrow(), 1);
        assert!(slept().is_empty());
    }

    #[test]
    fn a_transient_failure_of_an_idempotent_call_is_retried_with_backoff() {
        let (call, made) = calls(vec![Err(server(502)), Err(server(429)), Ok(7)]);
        assert_eq!(three().run(Idempotent::Yes, call), Ok(7));
        assert_eq!(*made.borrow(), 3);
        assert_eq!(
            slept(),
            [Duration::from_millis(500), Duration::from_secs(1)]
        );
    }

    #[test]
    fn it_gives_up_after_its_attempts() {
        let (call, made) = calls(vec![Err(server(500)), Err(server(500)), Err(server(500))]);
        assert_eq!(three().run(Idempotent::Yes, call), Err(server(500)));
        assert_eq!(*made.borrow(), 3);
        slept();
    }

    #[test]
    fn a_call_that_is_not_idempotent_or_a_real_failure_is_not_retried() {
        let (call, made) = calls(vec![Err(server(503))]);
        assert_eq!(three().run(Idempotent::No, call), Err(server(503)));
        assert_eq!(*made.borrow(), 1);
        let (call, made) = calls(vec![Err(server(403))]);
        assert_eq!(three().run(Idempotent::Yes, call), Err(server(403)));
        assert_eq!(*made.borrow(), 1);
        assert!(slept().is_empty());
    }

    #[test]
    fn a_retry_after_is_honoured_up_to_its_cap() {
        let (call, _) = calls(vec![
            Err(RestError::answered("GET", "/x", 429, "[]").retrying_after(3)),
            Err(RestError::answered("GET", "/x", 429, "[]").retrying_after(60)),
            Ok(1),
        ]);
        assert_eq!(three().run(Idempotent::Yes, call), Ok(1));
        assert_eq!(slept(), [Duration::from_secs(3), Duration::from_secs(10)]);
    }
}
