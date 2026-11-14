//! When to try a failed model call again. Pure: no sleeping, no clock, no providers.

/// Reads a `Retry-After` header given in seconds. The HTTP-date form is ignored.
pub fn retry_after_secs(header: &str) -> Option<u64> {
    header.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_is_read_only_in_seconds() {
        assert_eq!(retry_after_secs("7"), Some(7));
        assert_eq!(retry_after_secs(" 7 "), Some(7));
        assert_eq!(retry_after_secs("Wed, 21 Oct 2026 07:28:00 GMT"), None);
        assert_eq!(retry_after_secs("-1"), None);
    }
}
