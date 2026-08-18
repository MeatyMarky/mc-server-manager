//! One shared rate limiter for every API that publishes a budget.
//!
//! Modrinth answers with `X-Ratelimit-Remaining` and `X-Ratelimit-Reset`, and
//! returns 429 with `Retry-After` when a client ignores them. This keeps one
//! limiter per host so parallel searches, dependency resolution and update
//! checks share the same budget rather than each thinking it has the whole one.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Requests kept in reserve. Dependency resolution can fan out, and running the
/// budget to exactly zero means the next user action fails instead of waiting.
const RESERVE: i64 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub remaining: i64,
    /// Seconds until the budget resets.
    pub reset_in: u64,
}

/// What the limiter decided to do before the next request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Send it now.
    Proceed,
    /// Wait this long first.
    Wait(Duration),
}

#[derive(Debug, Default)]
struct HostState {
    budget: Option<Budget>,
    /// When the budget was observed, so `reset_in` can be aged.
    observed: Option<Instant>,
    /// Set by a 429 until its `Retry-After` has passed.
    blocked_until: Option<Instant>,
}

#[derive(Default)]
pub struct RateLimiter {
    hosts: Mutex<HashMap<String, HostState>>,
}

impl RateLimiter {
    /// Decides what to do before sending a request to `host`.
    pub fn decide(&self, host: &str, now: Instant) -> Decision {
        let Ok(hosts) = self.hosts.lock() else {
            return Decision::Proceed;
        };
        let Some(state) = hosts.get(host) else {
            return Decision::Proceed;
        };

        // A 429 is the hard signal: wait it out, whatever the counters say.
        if let Some(until) = state.blocked_until {
            if until > now {
                return Decision::Wait(until - now);
            }
        }

        let (Some(budget), Some(observed)) = (state.budget, state.observed) else {
            return Decision::Proceed;
        };
        if budget.remaining > RESERVE {
            return Decision::Proceed;
        }

        // Out of budget: wait for the window to roll over.
        let elapsed = now.saturating_duration_since(observed);
        let window = Duration::from_secs(budget.reset_in);
        if elapsed >= window {
            Decision::Proceed
        } else {
            Decision::Wait(window - elapsed)
        }
    }

    /// Records what a response said about the remaining budget.
    pub fn observe(&self, host: &str, budget: Budget, now: Instant) {
        if let Ok(mut hosts) = self.hosts.lock() {
            let state = hosts.entry(host.to_string()).or_default();
            state.budget = Some(budget);
            state.observed = Some(now);
            if budget.remaining > RESERVE {
                state.blocked_until = None;
            }
        }
    }

    /// Records a 429 and how long the server asked us to wait.
    pub fn observe_throttled(&self, host: &str, retry_after: Duration, now: Instant) {
        if let Ok(mut hosts) = self.hosts.lock() {
            let state = hosts.entry(host.to_string()).or_default();
            state.blocked_until = Some(now + retry_after);
            state.budget = Some(Budget {
                remaining: 0,
                reset_in: retry_after.as_secs(),
            });
            state.observed = Some(now);
        }
    }

    /// Waits until the limiter is willing to let a request through.
    pub async fn acquire(&self, host: &str) {
        loop {
            match self.decide(host, Instant::now()) {
                Decision::Proceed => return,
                Decision::Wait(delay) => {
                    // Cap a single sleep so a nonsense header cannot hang the UI.
                    let delay = delay.min(Duration::from_secs(60));
                    tracing::debug!(host, seconds = delay.as_secs(), "rate limited; waiting");
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    pub fn budget(&self, host: &str) -> Option<Budget> {
        self.hosts
            .lock()
            .ok()
            .and_then(|hosts| hosts.get(host).and_then(|state| state.budget))
    }
}

/// Reads the budget out of response headers, when the API publishes one.
pub fn budget_from_headers(headers: &reqwest::header::HeaderMap) -> Option<Budget> {
    let number = |name: &str| -> Option<i64> {
        headers
            .get(name)?
            .to_str()
            .ok()?
            .trim()
            .parse::<i64>()
            .ok()
    };

    let remaining = number("x-ratelimit-remaining")?;
    let reset_in = number("x-ratelimit-reset").unwrap_or(60).max(0) as u64;
    Some(Budget {
        remaining,
        reset_in,
    })
}

/// `Retry-After` in seconds, defaulting to a minute when the header is missing
/// or nonsense.
pub fn retry_after(headers: &reqwest::header::HeaderMap) -> Duration {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(60))
        .clamp(Duration::from_secs(1), Duration::from_secs(300))
}

/// The host part of a URL, which is what the budget is tracked against.
pub fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or(url)
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_host_proceeds() {
        let limiter = RateLimiter::default();
        assert_eq!(
            limiter.decide("api.modrinth.com", Instant::now()),
            Decision::Proceed
        );
    }

    #[test]
    fn plenty_of_budget_proceeds() {
        let limiter = RateLimiter::default();
        let now = Instant::now();
        limiter.observe(
            "api.modrinth.com",
            Budget {
                remaining: 250,
                reset_in: 60,
            },
            now,
        );
        assert_eq!(limiter.decide("api.modrinth.com", now), Decision::Proceed);
    }

    #[test]
    fn an_exhausted_budget_waits_for_the_window_to_roll_over() {
        let limiter = RateLimiter::default();
        let now = Instant::now();
        limiter.observe(
            "api.modrinth.com",
            Budget {
                remaining: 1,
                reset_in: 30,
            },
            now,
        );

        match limiter.decide("api.modrinth.com", now) {
            Decision::Wait(delay) => assert!(delay <= Duration::from_secs(30)),
            other => panic!("expected a wait, got {other:?}"),
        }

        // Once the window has passed, requests flow again.
        assert_eq!(
            limiter.decide("api.modrinth.com", now + Duration::from_secs(31)),
            Decision::Proceed
        );
    }

    #[test]
    fn a_429_blocks_until_retry_after_even_with_budget_left() {
        let limiter = RateLimiter::default();
        let now = Instant::now();
        limiter.observe(
            "api.modrinth.com",
            Budget {
                remaining: 300,
                reset_in: 60,
            },
            now,
        );
        limiter.observe_throttled("api.modrinth.com", Duration::from_secs(10), now);

        assert!(matches!(
            limiter.decide("api.modrinth.com", now),
            Decision::Wait(_)
        ));
        assert_eq!(
            limiter.decide("api.modrinth.com", now + Duration::from_secs(11)),
            Decision::Proceed
        );
    }

    #[test]
    fn budgets_are_tracked_per_host() {
        let limiter = RateLimiter::default();
        let now = Instant::now();
        limiter.observe(
            "api.modrinth.com",
            Budget {
                remaining: 0,
                reset_in: 60,
            },
            now,
        );
        assert!(matches!(
            limiter.decide("api.modrinth.com", now),
            Decision::Wait(_)
        ));
        assert_eq!(limiter.decide("cdn.modrinth.com", now), Decision::Proceed);
    }

    #[test]
    fn headers_are_read_when_present_and_ignored_when_not() {
        let mut headers = reqwest::header::HeaderMap::new();
        assert_eq!(budget_from_headers(&headers), None);

        headers.insert("x-ratelimit-remaining", "42".parse().unwrap());
        headers.insert("x-ratelimit-reset", "17".parse().unwrap());
        assert_eq!(
            budget_from_headers(&headers),
            Some(Budget {
                remaining: 42,
                reset_in: 17
            })
        );

        // A missing reset falls back to a minute rather than to zero.
        let mut partial = reqwest::header::HeaderMap::new();
        partial.insert("x-ratelimit-remaining", "5".parse().unwrap());
        assert_eq!(budget_from_headers(&partial).unwrap().reset_in, 60);
    }

    #[test]
    fn retry_after_is_clamped_to_something_sane() {
        let mut headers = reqwest::header::HeaderMap::new();
        assert_eq!(retry_after(&headers), Duration::from_secs(60));

        headers.insert(reqwest::header::RETRY_AFTER, "5".parse().unwrap());
        assert_eq!(retry_after(&headers), Duration::from_secs(5));

        headers.insert(reqwest::header::RETRY_AFTER, "99999".parse().unwrap());
        assert_eq!(retry_after(&headers), Duration::from_secs(300));

        headers.insert(reqwest::header::RETRY_AFTER, "not a number".parse().unwrap());
        assert_eq!(retry_after(&headers), Duration::from_secs(60));
    }

    #[test]
    fn hosts_come_from_urls() {
        assert_eq!(host_of("https://api.modrinth.com/v2/search"), "api.modrinth.com");
        assert_eq!(host_of("https://CDN.Modrinth.com/data/x"), "cdn.modrinth.com");
        assert_eq!(host_of("nonsense"), "nonsense");
    }

    #[tokio::test]
    async fn acquire_returns_immediately_when_there_is_budget() {
        let limiter = RateLimiter::default();
        limiter.observe(
            "api.modrinth.com",
            Budget {
                remaining: 100,
                reset_in: 60,
            },
            Instant::now(),
        );

        let started = Instant::now();
        limiter.acquire("api.modrinth.com").await;
        assert!(started.elapsed() < Duration::from_millis(200));
    }
}
