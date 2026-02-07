use crate::error::{NexusError, Result};
use std::time::Duration;

/// Maximum backoff cap to prevent excessively long waits.
const MAX_DELAY: Duration = Duration::from_secs(30);

/// Determine whether an error is retryable based on its display string.
///
/// Retryable patterns: rate limiting (429), server errors (500, 502, 503, 504),
/// timeouts, and connection issues.
///
/// Non-retryable patterns: client errors (400, 401, 403, 404), invalid input,
/// and unauthorized access.
fn is_retryable(err: &NexusError) -> bool {
    let msg = err.to_string().to_lowercase();

    // Non-retryable patterns take priority
    let non_retryable = ["400", "401", "403", "404", "invalid", "unauthorized"];
    for pattern in &non_retryable {
        if msg.contains(pattern) {
            return false;
        }
    }

    // Retryable patterns
    let retryable = [
        "429",
        "500",
        "502",
        "503",
        "504",
        "timeout",
        "connection refused",
        "connection reset",
    ];
    for pattern in &retryable {
        if msg.contains(pattern) {
            return true;
        }
    }

    false
}

/// Retry an async operation with exponential backoff.
///
/// Starts with `initial_delay` and doubles it each attempt, capping at 30 seconds.
/// Only retries errors classified as transient (rate limits, server errors,
/// timeouts, connection issues). Non-retryable errors are returned immediately.
///
/// # Arguments
/// * `max_retries` - Maximum number of retry attempts (0 means execute once with no retries)
/// * `initial_delay` - The delay before the first retry
/// * `f` - The async closure to retry
pub async fn retry_with_backoff<F, Fut, T>(
    max_retries: u32,
    initial_delay: Duration,
    f: F,
) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut delay = initial_delay;

    for attempt in 0..=max_retries {
        match f().await {
            Ok(val) => return Ok(val),
            Err(err) => {
                // If we've exhausted all retries, return the error
                if attempt == max_retries {
                    return Err(err);
                }

                // If the error is not retryable, return immediately
                if !is_retryable(&err) {
                    return Err(err);
                }

                // Log the retry attempt
                eprintln!(
                    "[retry] Attempt {}/{} failed ({}), retrying in {:?}...",
                    attempt + 1,
                    max_retries + 1,
                    err,
                    delay,
                );

                tokio::time::sleep(delay).await;

                // Double the delay for exponential backoff, capped at MAX_DELAY
                delay = (delay * 2).min(MAX_DELAY);
            }
        }
    }

    // This is unreachable due to the loop structure, but satisfies the compiler.
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn test_succeeds_first_try() {
        let result = retry_with_backoff(3, Duration::from_millis(1), || async {
            Ok::<_, NexusError>(42)
        })
        .await;

        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_retries_on_retryable_error() {
        let counter = AtomicU32::new(0);

        let result = retry_with_backoff(3, Duration::from_millis(1), || {
            let attempt = counter.fetch_add(1, Ordering::SeqCst);
            async move {
                if attempt < 2 {
                    Err(NexusError::ApiRequest("503 service unavailable".into()))
                } else {
                    Ok(99)
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), 99);
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_no_retry_on_non_retryable_error() {
        let counter = AtomicU32::new(0);

        let result = retry_with_backoff(3, Duration::from_millis(1), || {
            counter.fetch_add(1, Ordering::SeqCst);
            async { Err::<i32, _>(NexusError::ApiRequest("401 unauthorized".into())) }
        })
        .await;

        assert!(result.is_err());
        // Should only be called once (no retries for 401)
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_exhausts_retries() {
        let counter = AtomicU32::new(0);

        let result = retry_with_backoff(2, Duration::from_millis(1), || {
            counter.fetch_add(1, Ordering::SeqCst);
            async { Err::<i32, _>(NexusError::ApiRequest("500 internal server error".into())) }
        })
        .await;

        assert!(result.is_err());
        // Initial attempt + 2 retries = 3 total
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn test_retryable_classification() {
        // Retryable errors
        assert!(is_retryable(&NexusError::ApiRequest("429 Too Many Requests".into())));
        assert!(is_retryable(&NexusError::ApiRequest("500 Internal Server Error".into())));
        assert!(is_retryable(&NexusError::ApiRequest("502 Bad Gateway".into())));
        assert!(is_retryable(&NexusError::ApiRequest("503 Service Unavailable".into())));
        assert!(is_retryable(&NexusError::ApiRequest("504 Gateway Timeout".into())));
        assert!(is_retryable(&NexusError::ApiRequest("connection refused".into())));
        assert!(is_retryable(&NexusError::ApiRequest("connection reset".into())));
        assert!(is_retryable(&NexusError::ApiRequest("timeout waiting for response".into())));

        // Non-retryable errors
        assert!(!is_retryable(&NexusError::ApiRequest("400 Bad Request".into())));
        assert!(!is_retryable(&NexusError::ApiRequest("401 Unauthorized".into())));
        assert!(!is_retryable(&NexusError::ApiRequest("403 Forbidden".into())));
        assert!(!is_retryable(&NexusError::ApiRequest("404 Not Found".into())));
        assert!(!is_retryable(&NexusError::ApiRequest("invalid API key".into())));
        assert!(!is_retryable(&NexusError::ApiRequest("unauthorized access".into())));

        // Unknown errors are not retried
        assert!(!is_retryable(&NexusError::ApiRequest("some random error".into())));
    }
}
