//! When to try a failed model call again. Pure: no sleeping, no clock, no providers.

use std::time::Duration;

/// Retries after the first attempt, so a call is sent at most `MAX_RETRIES + 1` times.
pub const MAX_RETRIES: u32 = 3;
/// The first wait; each later one doubles.
const FIRST_WAIT: Duration = Duration::from_secs(1);
/// Random extra on each wait, as a share of it, so many clients don't retry in step.
const MAX_JITTER: f64 = 0.25;
/// The longest server-requested `Retry-After` that is honored.
const MAX_RETRY_AFTER_SECS: u64 = 30;

/// The waits between attempts: 1s, 2s, 4s, each plus up to 25% jitter, then none.
/// `jitter` returns a fraction in `[0, 1)`; values outside are clamped.
pub struct Backoff<J: FnMut() -> f64> {
    retries: u32,
    jitter: J,
}

impl<J: FnMut() -> f64> Backoff<J> {
    pub fn new(jitter: J) -> Self {
        Self { retries: 0, jitter }
    }

    /// The wait before the next retry, or `None` once retries are used up.
    /// A server's `Retry-After` (seconds) replaces the computed wait.
    pub fn next(&mut self, retry_after: Option<u64>) -> Option<Duration> {
        if self.retries >= MAX_RETRIES { return None; }
        let base = FIRST_WAIT * 2u32.pow(self.retries);
        self.retries += 1;
        Some(match retry_after {
            Some(secs) => Duration::from_secs(secs.min(MAX_RETRY_AFTER_SECS)),
            None => base.mul_f64(1.0 + MAX_JITTER * (self.jitter)().clamp(0.0, 1.0)),
        })
    }
}

/// Reads a `Retry-After` header given in seconds. The HTTP-date form is ignored.
pub fn retry_after_secs(header: &str) -> Option<u64> {
    header.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn waits(jitter: f64, retry_after: Option<u64>) -> Vec<Duration> {
        let mut backoff = Backoff::new(move || jitter);
        std::iter::from_fn(|| backoff.next(retry_after)).collect()
    }

    #[test]
    fn waits_double_from_one_second_three_times() {
        assert_eq!(waits(0.0, None), [1, 2, 4].map(Duration::from_secs));
    }

    #[test]
    fn jitter_adds_at_most_a_quarter() {
        for (wait, base) in waits(0.999, None).into_iter().zip([1.0, 2.0, 4.0]) {
            assert!(wait.as_secs_f64() > base && wait.as_secs_f64() < base * 1.25, "{wait:?}");
        }
        // A source outside [0, 1) can't stretch the wait further.
        assert_eq!(waits(7.0, None)[0], Duration::from_millis(1250));
    }

    #[test]
    fn retry_after_replaces_the_wait_and_is_capped() {
        assert_eq!(waits(0.5, Some(3)), [3, 3, 3].map(Duration::from_secs));
        assert_eq!(waits(0.5, Some(3600)), [30, 30, 30].map(Duration::from_secs));
    }

    #[test]
    fn retry_after_is_read_only_in_seconds() {
        assert_eq!(retry_after_secs("7"), Some(7));
        assert_eq!(retry_after_secs(" 7 "), Some(7));
        assert_eq!(retry_after_secs("Wed, 21 Oct 2026 07:28:00 GMT"), None);
        assert_eq!(retry_after_secs("-1"), None);
    }
}
