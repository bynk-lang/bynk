//! #4098: waiting for Cloudflare to show what a deploy has just created.
//!
//! Straight after a create, the account can lag behind it: a new queue is missing from the
//! queues listing for a few seconds, and a new Worker's lookup answers `10007` ("does not exist").
//! wrangler then fails on that lag (it creates the queue again and is refused with 11009, or skips
//! the triggers step), though a re-run seconds later succeeds. So the deploy polls a read until
//! what it created is visible, before it carries on.
//!
//! This is not the client's retry (ADR 0888, Decision H): no failed request is repeated, and only a
//! read is asked again, for something this run is known to have created. A poll that fails (a
//! denied token, a 5xx, the network) fails the deploy at once, with that failure.

use std::time::Duration;

use super::vectorize::Pacing;

/// How a deploy waits for what it just created to be visible: the first delay, the longest delay
/// (each double the last, up to this) and when to give up.
///
/// The lag on a scratch account was a few seconds, and a re-run about 20 seconds later succeeded
/// (#4098), so 30 seconds covers what was seen with room to spare, while a deploy whose platform
/// is truly stuck still says so within half a minute. The live check of a fresh-account first
/// deploy is to confirm this figure.
pub(crate) const VISIBILITY_PACING: Pacing = Pacing {
    first: Duration::from_secs(1),
    longest: Duration::from_secs(8),
    timeout: Duration::from_secs(30),
};

/// Ask `seen` until it answers `true`, sleeping between asks with `sleep` as `pacing` says:
/// `Ok(true)` once seen, `Ok(false)` when `pacing.timeout` of sleeping has passed without it, and
/// the first error `seen` returns. The time counted is the time slept, so a test's `sleep`
/// need not sleep.
pub(crate) fn wait_until<E>(
    mut seen: impl FnMut() -> Result<bool, E>,
    pacing: Pacing,
    sleep: &mut dyn FnMut(Duration),
) -> Result<bool, E> {
    let mut slept = Duration::ZERO;
    let mut delay = pacing.first;
    loop {
        if seen()? {
            return Ok(true);
        }
        if slept >= pacing.timeout {
            return Ok(false);
        }
        let step = delay.min(pacing.timeout - slept);
        sleep(step);
        slept += step;
        delay = (delay * 2).min(pacing.longest);
    }
}

/// What the deploy says when the wait runs out: the platform's lag, and that a re-run finishes.
pub(crate) fn lagging(what: &str, pacing: Pacing, rerun: &str) -> String {
    format!(
        "Cloudflare has not shown {what} {} s after it was created: the platform is lagging behind the create. Run `{rerun}` again to finish the deploy; nothing more is created",
        pacing.timeout.as_secs()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(answers: &[Result<bool, &'static str>]) -> (Result<bool, &'static str>, Vec<u64>) {
        let mut answers = answers.iter().copied();
        let mut slept = Vec::new();
        let result = wait_until(
            || answers.next().unwrap_or(Ok(false)),
            VISIBILITY_PACING,
            &mut |d| slept.push(d.as_secs()),
        );
        (result, slept)
    }

    #[test]
    fn what_is_visible_at_once_is_not_waited_for() {
        assert_eq!(run(&[Ok(true)]), (Ok(true), vec![]));
    }

    #[test]
    fn a_lagging_create_is_waited_for_with_backoff() {
        assert_eq!(
            run(&[Ok(false), Ok(false), Ok(false), Ok(true)]),
            (Ok(true), vec![1, 2, 4])
        );
    }

    #[test]
    fn the_wait_gives_up_at_the_cap() {
        let (result, slept) = run(&[]);
        assert_eq!(result, Ok(false));
        assert_eq!(slept, [1, 2, 4, 8, 8, 7]);
        assert_eq!(slept.iter().sum::<u64>(), 30);
    }

    #[test]
    fn a_failed_poll_fails_at_once() {
        assert_eq!(run(&[Ok(false), Err("denied")]), (Err("denied"), vec![1]));
    }

    #[test]
    fn the_lag_message_says_a_rerun_finishes() {
        assert_eq!(
            lagging("the queue `jobs`", VISIBILITY_PACING, "bynk deploy"),
            "Cloudflare has not shown the queue `jobs` 30 s after it was created: the platform is lagging behind the create. Run `bynk deploy` again to finish the deploy; nothing more is created"
        );
    }
}
