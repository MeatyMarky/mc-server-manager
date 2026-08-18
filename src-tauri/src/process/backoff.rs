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
    clean_exit: bool,
    recent_crashes: i64,
    restart_max: i64,
    restart_window_s: i64,
) -> RestartDecision {
    if clean_exit {
        return RestartDecision::CleanExit;
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

/// A non-zero exit, or a zero exit that the user did not ask for, counts as a
/// crash. A server stopped through the UI exits 0 *and* was requested.
pub fn is_crash(exit_code: Option<i32>, stop_requested: bool) -> bool {
    if stop_requested {
        return false;
    }
    !matches!(exit_code, Some(0))
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
        assert_eq!(decide(true, true, 0, 3, 600), RestartDecision::CleanExit);
    }

    #[test]
    fn auto_restart_off_means_nothing_happens() {
        assert_eq!(decide(false, false, 0, 3, 600), RestartDecision::Disabled);
    }

    #[test]
    fn the_first_crash_restarts_after_the_base_delay() {
        assert_eq!(
            decide(true, false, 0, 3, 600),
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
            decide(true, false, 3, 3, 600),
            RestartDecision::GaveUp {
                attempts: 4,
                window_secs: 600
            }
        );
        // And the attempt right before the cap still restarts, with a long wait.
        assert_eq!(
            decide(true, false, 2, 3, 600),
            RestartDecision::Restart {
                delay: Duration::from_secs(20),
                attempt: 3
            }
        );
    }

    #[test]
    fn a_cap_of_zero_disables_restarting_entirely() {
        assert!(matches!(
            decide(true, false, 0, 0, 600),
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
}
