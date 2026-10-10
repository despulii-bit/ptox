use super::{temp_path, verify_sha256, FetchError, FetchRequest, Fetched, Fetcher, ProgressFn};
use async_trait::async_trait;
use futures::StreamExt;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// A descriptive default; many hosts (Wikimedia, etc.) reject requests without one.
const DEFAULT_USER_AGENT: &str = concat!(
    "ptox/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/despulii-bit/ptox)"
);

/// How many times an HTTP GET is attempted and how long to wait between tries.
///
/// The default is **no retry**: exactly one attempt. Build one with
/// [`RetryPolicy::new`] to allow retries, or [`RetryPolicy::none`] to be
/// explicit. Only transient failures are retried — connection errors, mid-stream
/// read errors, and `408`/`429`/`5xx` responses — never `4xx` client errors or a
/// hash mismatch. GETs are idempotent, so replaying them is safe.
///
/// ```
/// use ptox::RetryPolicy;
/// use std::time::Duration;
///
/// let policy = RetryPolicy::new(3).with_base_delay(Duration::from_millis(200));
/// assert_eq!(policy.max_attempts(), 3);
/// assert_eq!(policy.base_delay(), Duration::from_millis(200));
/// ```
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    max_attempts: u32,
    base_delay: Duration,
    max_delay: Duration,
}

impl RetryPolicy {
    /// The default delay before the first retry (doubles for each later one).
    pub const DEFAULT_BASE_DELAY: Duration = Duration::from_millis(200);
    /// The default cap on the delay between attempts.
    pub const DEFAULT_MAX_DELAY: Duration = Duration::from_secs(10);

    /// Allow up to `max_attempts` total attempts (a value of `1`, or `0`, means
    /// a single attempt and no retry), waiting [`RetryPolicy::DEFAULT_BASE_DELAY`]
    /// before the first retry and doubling each time up to
    /// [`RetryPolicy::DEFAULT_MAX_DELAY`].
    pub fn new(max_attempts: u32) -> Self {
        Self {
            max_attempts: max_attempts.max(1),
            base_delay: Self::DEFAULT_BASE_DELAY,
            max_delay: Self::DEFAULT_MAX_DELAY,
        }
    }

    /// A policy that makes exactly one attempt (the default).
    pub fn none() -> Self {
        Self::new(1)
    }

    /// Total attempts, including the first.
    pub fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    /// The delay before the first retry.
    pub fn base_delay(&self) -> Duration {
        self.base_delay
    }

    /// The upper bound on the delay between attempts.
    pub fn max_delay(&self) -> Duration {
        self.max_delay
    }

    /// Override the delay before the first retry; later retries double it.
    pub fn with_base_delay(mut self, base_delay: Duration) -> Self {
        self.base_delay = base_delay;
        self
    }

    /// Override the cap on the delay between attempts.
    pub fn with_max_delay(mut self, max_delay: Duration) -> Self {
        self.max_delay = max_delay;
        self
    }

    /// The delay to wait after `failed_attempt` (1-based) before trying again:
    /// `base_delay * 2^(failed_attempt - 1)`, capped at [`RetryPolicy::max_delay`].
    fn delay_for(&self, failed_attempt: u32) -> Duration {
        let exponent = failed_attempt.saturating_sub(1).min(31);
        let factor = 1u32 << exponent;
        self.base_delay.saturating_mul(factor).min(self.max_delay)
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::none()
    }
}

/// An HTTP status worth retrying: request timeout, rate limiting, or server
/// error. Client errors (`4xx`) are permanent — the request will not improve.
fn is_transient_status(status: reqwest::StatusCode) -> bool {
    status.is_server_error()
        || status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
}

/// Classifies a single attempt's failure so the retry loop knows whether to try
/// again.
enum AttemptError {
    /// A failure that might succeed on another attempt.
    Transient(FetchError),
    /// A failure retrying cannot fix (bad request, hash mismatch, disk error).
    Permanent(FetchError),
}

/// Fetches `http`/`https` URLs. Writes atomically and verifies sha256.
pub struct HttpFetcher {
    client: reqwest::Client,
    schemes: Vec<String>,
    retry: RetryPolicy,
}

impl HttpFetcher {
    /// A fetcher for `http`/`https` using the default `User-Agent` and no retry.
    pub fn new() -> Self {
        Self::with_user_agent(DEFAULT_USER_AGENT)
    }

    /// Override the `User-Agent` sent with requests.
    pub fn with_user_agent(user_agent: &str) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(user_agent)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            client,
            schemes: vec!["http".to_owned(), "https".to_owned()],
            retry: RetryPolicy::default(),
        }
    }

    /// Opt in to retrying transient failures with `policy`.
    ///
    /// Retries are off by default (one attempt). Because GETs are idempotent, a
    /// client talking to a flaky mirror can enable a bounded exponential backoff:
    ///
    /// ```no_run
    /// use ptox::{HttpFetcher, RetryPolicy};
    /// use std::time::Duration;
    ///
    /// let fetcher = HttpFetcher::new().with_retry(
    ///     RetryPolicy::new(3).with_base_delay(Duration::from_millis(200)),
    /// );
    /// ```
    pub fn with_retry(mut self, policy: RetryPolicy) -> Self {
        self.retry = policy;
        self
    }

    /// The retry policy in effect.
    pub fn retry_policy(&self) -> &RetryPolicy {
        &self.retry
    }
}

impl Default for HttpFetcher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Fetcher for HttpFetcher {
    fn name(&self) -> &str {
        "http"
    }

    fn schemes(&self) -> &[String] {
        &self.schemes
    }

    async fn fetch(&self, request: &FetchRequest) -> Result<Fetched, FetchError> {
        self.fetch_with_progress(request, &|_: u64| {}).await
    }

    async fn fetch_with_progress(
        &self,
        request: &FetchRequest,
        progress: &ProgressFn<'_>,
    ) -> Result<Fetched, FetchError> {
        let attempts = self.retry.max_attempts();
        // A failed attempt may report bytes before failing; clamp later reports
        // to the high-water mark so a retry never makes progress regress.
        let high_water = AtomicU64::new(0);
        let monotonic = |bytes: u64| {
            let previous = high_water.fetch_max(bytes, Ordering::Relaxed);
            if bytes > previous {
                progress(bytes);
            }
        };

        let mut last_error = None;
        for attempt in 1..=attempts {
            match self.fetch_once(request, &monotonic).await {
                Ok(fetched) => return Ok(fetched),
                Err(AttemptError::Transient(error)) if attempt < attempts => {
                    last_error = Some(error);
                    let delay = self.retry.delay_for(attempt);
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                }
                Err(AttemptError::Transient(error)) | Err(AttemptError::Permanent(error)) => {
                    return Err(error)
                }
            }
        }
        Err(last_error.unwrap_or_else(|| FetchError::Network("retry exhausted".to_owned())))
    }
}

impl HttpFetcher {
    /// Perform one HTTP GET + download, classifying failures as transient or
    /// permanent for the retry loop. Removes the `.part` file on any failure, so
    /// a retry starts from a clean destination.
    async fn fetch_once(
        &self,
        request: &FetchRequest,
        progress: &ProgressFn<'_>,
    ) -> Result<Fetched, AttemptError> {
        let response = self
            .client
            .get(&request.address.uri)
            .send()
            .await
            .map_err(|error| AttemptError::Transient(FetchError::Network(error.to_string())))?;
        let status = response.status();
        if !status.is_success() {
            let error = FetchError::Network(format!("HTTP {status} for {}", request.address.uri));
            return Err(if is_transient_status(status) {
                AttemptError::Transient(error)
            } else {
                AttemptError::Permanent(error)
            });
        }

        let temp = temp_path(&request.destination);
        match download(response, &temp, request, progress).await {
            Ok(fetched) => Ok(fetched),
            Err(error) => {
                // Never leave a partial download behind; the next attempt (if
                // any) recreates the file from scratch.
                let _ = tokio::fs::remove_file(&temp).await;
                Err(match error {
                    network @ FetchError::Network(_) => AttemptError::Transient(network),
                    other => AttemptError::Permanent(other),
                })
            }
        }
    }
}

/// Stream `response` to `temp`, verify its hash, then rename it into place.
async fn download(
    response: reqwest::Response,
    temp: &Path,
    request: &FetchRequest,
    progress: &ProgressFn<'_>,
) -> Result<Fetched, FetchError> {
    if let Some(parent) = temp.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut file = tokio::fs::File::create(temp).await?;
    let mut hasher = Sha256::new();
    let mut stream = response.bytes_stream();
    let mut total = 0u64;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| FetchError::Network(error.to_string()))?;
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
        total += chunk.len() as u64;
        progress(total);
    }
    file.flush().await?;

    let digest = hex::encode(hasher.finalize());
    verify_sha256(request.expected_sha256.as_deref(), &digest)?;
    tokio::fs::rename(temp, &request.destination).await?;

    Ok(Fetched {
        path: request.destination.clone(),
        bytes: total,
        sha256: Some(digest),
        address: request.address.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_defaults_to_a_single_attempt() {
        assert_eq!(RetryPolicy::default().max_attempts(), 1);
        assert_eq!(RetryPolicy::none().max_attempts(), 1);
        assert_eq!(RetryPolicy::new(0).max_attempts(), 1);
        assert_eq!(RetryPolicy::new(4).max_attempts(), 4);
    }

    #[test]
    fn retry_backoff_doubles_then_caps() {
        let policy = RetryPolicy::new(5)
            .with_base_delay(Duration::from_millis(100))
            .with_max_delay(Duration::from_millis(250));
        assert_eq!(policy.delay_for(1), Duration::from_millis(100));
        assert_eq!(policy.delay_for(2), Duration::from_millis(200));
        assert_eq!(policy.delay_for(3), Duration::from_millis(250)); // capped
        assert_eq!(policy.delay_for(100), Duration::from_millis(250));
    }

    #[test]
    fn only_server_side_statuses_are_transient() {
        use reqwest::StatusCode;
        for status in [
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::GATEWAY_TIMEOUT,
            StatusCode::REQUEST_TIMEOUT,
            StatusCode::TOO_MANY_REQUESTS,
        ] {
            assert!(is_transient_status(status), "{status} should be transient");
        }
        for status in [
            StatusCode::OK,
            StatusCode::BAD_REQUEST,
            StatusCode::NOT_FOUND,
            StatusCode::FORBIDDEN,
        ] {
            assert!(!is_transient_status(status), "{status} should be permanent");
        }
    }
}
