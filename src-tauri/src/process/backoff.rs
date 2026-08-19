//! Auto-restart policy.
//!
//! A server that crashes on boot must not spin: attempts are counted inside a
//! rolling window, the delay grows exponentially, and once the cap is reached
//! restarting stops until a human intervenes. Pure functions so the policy can
//! be tested without waiting for real time to pass.

use std::time::Duration;

/// Base delay before the first restart attempt.
pub const BASE_DELAY_SECS: u64 = 5;
/// No single wait grows past this.
pub const MAX_DELAY_SECS: u64 = 300;

/// How a server process ended, which decides whether restarting it could help.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// The user asked for it.
    Requested,
    /// The process ended before the server ever reported being ready.
    ///
    /// Nothing about waiting changes a bad `-Xmx`, a missing jar or a port that
    /// is taken, so this must not be retried: the first attempt already carries
    /// the whole answer, and four more only bury it in the console.
    FailedStart,
    /// The server was running, then died. This is what backoff exists for.
    Crash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartDecision {
    /// Restart after waiting.
    Restart { delay: Duration, attempt: i64 },
    /// Auto-restart is switched off for this instance.
    Disabled,
    /// Too many crashes inside the window: stop trying.
    GaveUp { attempts: i64, window_secs: i64 },
    /// The server exited cleanly (a requested stop), so there is nothing to do.
    CleanExit,
    /// It never started. Retrying would repeat a deterministic failure.
    FailedStart,
}

/// Exponential backoff: 5s, 10s, 20s, 40s… capped at [`MAX_DELAY_SECS`].
pub fn delay_for_attempt(attempt: i64) -> Duration {
    let exponent = attempt.clamp(1, 16) as u32 - 1;
    let secs = BASE_DELAY_SECS
        .saturating_mul(2u64.saturating_pow(exponent))
        .min(MAX_DELAY_SECS);
    Duration::from_secs(secs)
}

/// Decides what to do after a server process exits.
///
/// `recent_crashes` counts crashes already recorded inside `restart_window_s`,
/// not counting the one being handled now.
pub fn decide(
    auto_restart: bool,
    exit: Exit,
    recent_crashes: i64,
    restart_max: i64,
    restart_window_s: i64,
) -> RestartDecision {
    match exit {
        Exit::Requested => return RestartDecision::CleanExit,
        // Checked before `auto_restart`, because the reason is worth reporting
        // whether or not the user has restarts switched on.
        Exit::FailedStart => return RestartDecision::FailedStart,
        Exit::Crash => {}
    }
    if !auto_restart {
        return RestartDecision::Disabled;
    }

    let attempt = recent_crashes + 1;
    if attempt > restart_max.max(0) {
        return RestartDecision::GaveUp {
            attempts: attempt,
            window_secs: restart_window_s,
        };
    }

    RestartDecision::Restart {
        delay: delay_for_attempt(attempt),
        attempt,
    }
}

/// The status an instance takes when its process ends.
///
/// A start that never finished leaves the same badge as a crash — the server is
/// not running and something went wrong — and never leaves "Starting" on screen
/// after the process is gone.
pub fn status_for(exit: Exit) -> crate::db::models::InstanceStatus {
    use crate::db::models::InstanceStatus;
    match exit {
        Exit::Requested => InstanceStatus::Stopped,
        Exit::FailedStart | Exit::Crash => InstanceStatus::Crashed,
    }
}

/// A non-zero exit, or a zero exit that the user did not ask for, counts as a
/// crash. A server stopped through the UI exits 0 *and* was requested.
pub fn is_crash(exit_code: Option<i32>, stop_requested: bool) -> bool {
    if stop_requested {
        return false;
    }
    !matches!(exit_code, Some(0))
}

/// Sorts an exit into the three cases that matter.
///
/// `reached_ready` is whether the server printed its "Done" line during this
/// run. Without it, the process died on the way up — a JVM that refused the
/// heap, a missing jar, a taken port — and the fix is always something the user
/// has to change.
pub fn classify(stop_requested: bool, reached_ready: bool) -> Exit {
    if stop_requested {
        Exit::Requested
    } else if !reached_ready {
        Exit::FailedStart
    } else {
        // Past the "Done" line, any exit nobody asked for is a crash —
        // including a clean exit 0 from an in-game `/stop` or a plugin, which
        // is exactly what auto-restart is meant to bring back.
        Exit::Crash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delays_grow_exponentially_and_then_stop_growing() {
        assert_eq!(delay_for_attempt(1), Duration::from_secs(5));
        assert_eq!(delay_for_attempt(2), Duration::from_secs(10));
        assert_eq!(delay_for_attempt(3), Duration::from_secs(20));
        assert_eq!(delay_for_attempt(4), Duration::from_secs(40));
        assert_eq!(delay_for_attempt(10), Duration::from_secs(MAX_DELAY_SECS));
        // Absurd input must not overflow.
        assert_eq!(delay_for_attempt(i64::MAX), Duration::from_secs(MAX_DELAY_SECS));
        assert_eq!(delay_for_attempt(0), Duration::from_secs(5));
    }

    #[test]
    fn a_clean_stop_never_restarts() {
        assert_eq!(
            decide(true, Exit::Requested, 0, 3, 600),
            RestartDecision::CleanExit
        );
    }

    #[test]
    fn auto_restart_off_means_nothing_happens() {
        assert_eq!(
            decide(false, Exit::Crash, 0, 3, 600),
            RestartDecision::Disabled
        );
    }

    #[test]
    fn the_first_crash_restarts_after_the_base_delay() {
        assert_eq!(
            decide(true, Exit::Crash, 0, 3, 600),
            RestartDecision::Restart {
                delay: Duration::from_secs(5),
                attempt: 1
            }
        );
    }

    #[test]
    fn a_server_that_crashes_instantly_gives_up_at_the_cap() {
        // Three crashes already inside the window, cap of three.
        assert_eq!(
            decide(true, Exit::Crash, 3, 3, 600),
            RestartDecision::GaveUp {
                attempts: 4,
                window_secs: 600
            }
        );
        // And the attempt right before the cap still restarts, with a long wait.
        assert_eq!(
            decide(true, Exit::Crash, 2, 3, 600),
            RestartDecision::Restart {
                delay: Duration::from_secs(20),
                attempt: 3
            }
        );
    }

    #[test]
    fn a_cap_of_zero_disables_restarting_entirely() {
        assert!(matches!(
            decide(true, Exit::Crash, 0, 0, 600),
            RestartDecision::GaveUp { .. }
        ));
    }

    #[test]
    fn crash_detection_follows_the_exit_code_unless_a_stop_was_requested() {
        assert!(is_crash(Some(1), false));
        assert!(is_crash(None, false), "killed by a signal counts as a crash");
        assert!(!is_crash(Some(0), false), "a clean exit is not a crash");
        // A stop the user asked for is never a crash, whatever the exit code.
        assert!(!is_crash(Some(143), true));
        assert!(!is_crash(Some(0), true));
    }

    #[test]
    fn a_process_that_never_reached_ready_is_a_failed_start() {
        // The reported case: a 32-bit JVM refusing -Xmx8192M exits 1 in under a
        // second, having printed only "Invalid maximum heap size".
        assert_eq!(classify(false, false), Exit::FailedStart);
        // Even an exit 0 before "Done" is a start that did not happen.
        assert_eq!(classify(false, false), Exit::FailedStart);
        // Once the server said Done, the same exit is a crash worth retrying.
        assert_eq!(classify(false, true), Exit::Crash);
        // And a stop the user asked for is neither, ready or not.
        assert_eq!(classify(true, true), Exit::Requested);
        assert_eq!(classify(true, false), Exit::Requested);
    }

    #[test]
    fn a_failed_start_is_never_retried() {
        // Waiting five seconds does not fix a bad -Xmx, a missing jar or a
        // taken port, so backoff must not touch this case at all.
        for recent in [0, 1, 5] {
            assert_eq!(
                decide(true, Exit::FailedStart, recent, 3, 600),
                RestartDecision::FailedStart,
                "recent crashes must not turn a failed start into a retry"
            );
        }

        // Not even with auto-restart switched on and a huge cap.
        assert_eq!(
            decide(true, Exit::FailedStart, 0, 100, 600),
            RestartDecision::FailedStart
        );
    }

    #[test]
    fn backoff_still_applies_to_a_server_that_was_actually_running() {
        assert_eq!(
            decide(true, Exit::Crash, 0, 3, 600),
            RestartDecision::Restart {
                delay: Duration::from_secs(5),
                attempt: 1
            }
        );
        assert_eq!(
            decide(true, Exit::Crash, 1, 3, 600),
            RestartDecision::Restart {
                delay: Duration::from_secs(10),
                attempt: 2
            }
        );
    }

    #[test]
    fn a_failed_start_leaves_the_same_badge_as_a_crash_never_starting() {
        use crate::db::models::InstanceStatus;

        assert_eq!(status_for(Exit::FailedStart), InstanceStatus::Crashed);
        assert_eq!(status_for(Exit::Crash), InstanceStatus::Crashed);
        assert_eq!(status_for(Exit::Requested), InstanceStatus::Stopped);

        // Whatever happens, the process is gone, so no outcome may leave the
        // instance looking like it is still coming up.
        for exit in [Exit::Requested, Exit::FailedStart, Exit::Crash] {
            assert_ne!(status_for(exit), InstanceStatus::Starting);
            assert_ne!(status_for(exit), InstanceStatus::Running);
        }
    }
}
