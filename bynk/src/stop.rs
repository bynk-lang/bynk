//! Stopping `bynk dev` from outside (#1955).
//!
//! Without a handler, a SIGTERM or SIGINT sent to `bynk dev` alone (by a CI
//! step, a test harness, an editor task) killed it on the spot, and its
//! wranglers and their `workerd`s ran on with their ports bound. A Ctrl-C in a
//! terminal never showed it, because the terminal signals the whole foreground
//! group, wranglers included.
//!
//! [`install`] replaces the default action with one that only records the
//! signal. `bynk dev`'s watch loop sees it on its next tick and stops its
//! contexts the way it does when one of them exits: SIGTERM to each wrangler,
//! then the sweep of what they leave behind ([`crate::sweep`]). The wranglers
//! stay in `bynk dev`'s process group, because the sweep finds orphaned
//! `workerd`s by that group and a Ctrl-C must still reach them all.
//!
//! The handler is one-shot (`SA_RESETHAND`): a second Ctrl-C while the
//! contexts are stopping takes the default action and ends `bynk dev` at once.
//!
//! Unix only. On Windows each wrangler's job object holds its tree (#1762), and
//! [`install`] does nothing.

use bynk_driver::emsg;
use std::sync::atomic::{AtomicI32, Ordering};

/// The signal received since [`install`], or 0 for none.
static RECEIVED: AtomicI32 = AtomicI32::new(0);

/// Record SIGINT and SIGTERM instead of dying of them, for [`received`] to
/// report. Children spawned afterwards are unaffected: `exec` resets a caught
/// signal to its default action.
pub fn install() {
    #[cfg(unix)]
    for sig in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: `record` only stores to an atomic, which is
        // async-signal-safe, and `action` is fully initialised before use.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = record as extern "C" fn(libc::c_int) as libc::sighandler_t;
            action.sa_flags = libc::SA_RESTART | libc::SA_RESETHAND;
            libc::sigemptyset(&mut action.sa_mask);
            if libc::sigaction(sig, &action, std::ptr::null_mut()) != 0 {
                emsg!(
                    "bynk dev: could not install a handler for signal {sig}; stopping \
                     `bynk dev` with it may leave its workerd processes running."
                );
            }
        }
    }
}

#[cfg(unix)]
extern "C" fn record(sig: libc::c_int) {
    RECEIVED.store(sig, Ordering::SeqCst);
}

/// The signal that asked `bynk dev` to stop, if one has arrived.
pub fn received() -> Option<i32> {
    match RECEIVED.load(Ordering::SeqCst) {
        0 => None,
        sig => Some(sig),
    }
}

/// #2055: SIGINT and SIGTERM recorded rather than fatal for as long as the
/// returned guard lives, then the dispositions in force before restored.
///
/// `bynk deploy` holds one while a first deploy's `wrangler deploy
/// --secrets-file` runs, so a Ctrl-C (which the terminal sends to the whole
/// foreground group, wrangler included) or a SIGTERM sent to `bynk` alone
/// cannot end `bynk` before it removes the secrets file. A caught signal is
/// reset to its default by `exec`, so wrangler itself still dies of them as it
/// would have. Unlike [`install`], the handler is not one-shot: a second
/// Ctrl-C must not end `bynk` with the file on disk either. Only SIGKILL can,
/// and the next deploy's sweep removes what that leaves.
///
/// Clears any earlier recorded signal, so [`received`] reports only what
/// arrived under this guard. On Windows it changes nothing.
#[must_use = "the signals are restored when the guard drops"]
pub(crate) fn defer() -> Deferred {
    RECEIVED.store(0, Ordering::SeqCst);
    #[cfg(unix)]
    {
        let mut previous: [Option<libc::sigaction>; 2] = [None, None];
        for (slot, sig) in previous.iter_mut().zip([libc::SIGINT, libc::SIGTERM]) {
            // SAFETY: `record` only stores to an atomic, which is
            // async-signal-safe; both structs are fully initialised before use,
            // and `old` is only kept when the call succeeded.
            unsafe {
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = record as extern "C" fn(libc::c_int) as libc::sighandler_t;
                action.sa_flags = libc::SA_RESTART;
                libc::sigemptyset(&mut action.sa_mask);
                let mut old: libc::sigaction = std::mem::zeroed();
                if libc::sigaction(sig, &action, &mut old) == 0 {
                    *slot = Some(old);
                }
            }
        }
        Deferred { previous }
    }
    #[cfg(not(unix))]
    Deferred {}
}

/// The guard [`defer`] returns. Dropping it puts back the SIGINT and SIGTERM
/// dispositions [`defer`] replaced.
pub(crate) struct Deferred {
    #[cfg(unix)]
    previous: [Option<libc::sigaction>; 2],
}

impl Drop for Deferred {
    fn drop(&mut self) {
        #[cfg(unix)]
        for (old, sig) in self.previous.iter().zip([libc::SIGINT, libc::SIGTERM]) {
            if let Some(old) = old {
                // SAFETY: `old` is the disposition `sigaction` returned.
                unsafe {
                    libc::sigaction(sig, old, std::ptr::null_mut());
                }
            }
        }
    }
}

/// Serialises the tests that change this process's signal dispositions or
/// signal it, which would otherwise race in the shared test binary.
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Undo [`install`] and forget any recorded signal, so a test that installs
/// the handler leaves nothing behind in the shared test process.
#[cfg(test)]
pub(crate) fn reset() {
    #[cfg(unix)]
    for sig in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: restoring the default disposition.
        unsafe {
            libc::signal(sig, libc::SIG_DFL);
        }
    }
    RECEIVED.store(0, Ordering::SeqCst);
}

/// The exit byte for a session stopped by `sig`, as [`crate::shell::exit_status_byte`]
/// maps a child's signal death: SIGINT is a clean stop (it is what Ctrl-C
/// sends), anything else `128 + sig`, the shell convention.
pub fn exit_byte(sig: i32) -> u8 {
    // SIGINT is 2 on every Unix.
    if sig == 2 {
        0
    } else {
        128u8.saturating_add(sig.clamp(0, 127) as u8)
    }
}

/// How to name `sig` in `bynk dev`'s stop notice.
pub fn name(sig: i32) -> String {
    match sig {
        2 => "SIGINT".to_string(),
        15 => "SIGTERM".to_string(),
        n => format!("signal {n}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The handler `sig` currently has.
    #[cfg(unix)]
    fn disposition(sig: libc::c_int) -> libc::sighandler_t {
        // SAFETY: a query; `current` is written by the call.
        unsafe {
            let mut current: libc::sigaction = std::mem::zeroed();
            assert_eq!(libc::sigaction(sig, std::ptr::null(), &mut current), 0);
            current.sa_sigaction
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_deferred_signal_is_recorded_and_the_old_disposition_comes_back() {
        // #2055: what keeps `bynk deploy` alive to remove a secrets file.
        let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let before = [disposition(libc::SIGINT), disposition(libc::SIGTERM)];
        {
            let _deferred = defer();
            assert_eq!(received(), None, "nothing carried over");
            // Without the handler this would end the test binary.
            // SAFETY: signalling this process, whose handler only records.
            unsafe {
                libc::raise(libc::SIGTERM);
                libc::raise(libc::SIGINT);
            }
            assert!(received().is_some(), "the signals were recorded");
            // SAFETY: as above; the handler is not one-shot.
            unsafe {
                libc::raise(libc::SIGINT);
            }
        }
        assert_eq!(
            [disposition(libc::SIGINT), disposition(libc::SIGTERM)],
            before,
            "the dispositions in force before are restored"
        );
        RECEIVED.store(0, Ordering::SeqCst);
    }

    #[test]
    fn a_signal_stop_exits_like_a_signalled_child() {
        assert_eq!(exit_byte(2), 0, "Ctrl-C is a clean stop");
        assert_eq!(exit_byte(15), 143);
    }
}
