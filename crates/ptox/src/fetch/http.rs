use super::{temp_path, verify_sha256, FetchError, FetchRequest, Fetched, Fetcher, ProgressFn};
use async_trait::async_trait;
use futures::StreamExt;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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

impl From<FetchError> for AttemptError {
    fn from(error: FetchError) -> Self {
        // A client/configuration error (e.g. an invalid proxy) is not something
        // a retry can fix.
        AttemptError::Permanent(error)
    }
}

/// A proxy to route HTTP requests through, for retrieval over overlay networks.
///
/// A thin wrapper over a proxy URL, accepted by [`HttpFetcher::with_proxy`]. The
/// URL scheme selects the transport:
///
/// - `http://` / `https://` — an HTTP proxy (e.g. i2pd's HTTP proxy at
///   `http://127.0.0.1:4444`);
/// - `socks5://` — a SOCKS5 proxy (e.g. Tor's `SocksPort`), resolving the host
///   locally;
/// - `socks5h://` — SOCKS5 with **remote** DNS resolution, which overlay names
///   (`.onion`, `.i2p`) require.
///
/// Requests made by an [`HttpFetcher`] configured with a proxy are sent *through*
/// it, so register such a fetcher only for the overlay scheme (a `socks5h://`
/// fetcher for `tor`, an `http://` fetcher for `i2p`) rather than for the shared
/// `http`/`https` registry entry.
///
/// ```
/// use ptox::ProxyConfig;
///
/// let tor = ProxyConfig::new("socks5h://127.0.0.1:9050");
/// assert_eq!(tor.url(), "socks5h://127.0.0.1:9050");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyConfig {
    url: String,
}

impl ProxyConfig {
    /// A proxy URL. The scheme picks the transport (`http`, `https`, `socks5`,
    /// or `socks5h`).
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into() }
    }

    /// The proxy URL.
    pub fn url(&self) -> &str {
        &self.url
    }
}

impl From<&str> for ProxyConfig {
    fn from(url: &str) -> Self {
        Self::new(url)
    }
}

impl From<String> for ProxyConfig {
    fn from(url: String) -> Self {
        Self::new(url)
    }
}

/// Fetches `http`/`https` URLs. Writes atomically and verifies sha256.
pub struct HttpFetcher {
    client: reqwest::Client,
    client_error: Option<String>,
    user_agent: Option<String>,
    connect_timeout: Option<Duration>,
    read_timeout: Option<Duration>,
    proxy: Option<ProxyConfig>,
    schemes: Vec<String>,
    retry: RetryPolicy,
    resume: bool,
    max_bytes: Option<u64>,
    deny_link_local: bool,
}

impl HttpFetcher {
    /// A fetcher for `http`/`https` using the default `User-Agent`.
    ///
    /// The client has **no timeouts** by default (reqwest's default), so a
    /// stalled connection or a server that stops sending can hang a fetch. Opt
    /// in with [`HttpFetcher::with_connect_timeout`] and
    /// [`HttpFetcher::with_read_timeout`]. Retries are off (one attempt) and
    /// resuming is off; see [`HttpFetcher::with_retry`] and
    /// [`HttpFetcher::with_resume`].
    pub fn new() -> Self {
        Self::with_user_agent(DEFAULT_USER_AGENT)
    }

    /// Build the underlying client from the current options. Returns an error
    /// (rather than silently dropping the proxy) if reqwest rejects the
    /// configuration, so a misconfigured proxy never leaks traffic direct.
    fn build_client(
        user_agent: Option<&str>,
        connect_timeout: Option<Duration>,
        read_timeout: Option<Duration>,
        proxy: Option<&ProxyConfig>,
    ) -> Result<reqwest::Client, reqwest::Error> {
        let mut builder = reqwest::Client::builder();
        if let Some(user_agent) = user_agent {
            builder = builder.user_agent(user_agent);
        }
        if let Some(timeout) = connect_timeout {
            builder = builder.connect_timeout(timeout);
        }
        if let Some(timeout) = read_timeout {
            builder = builder.read_timeout(timeout);
        }
        if let Some(proxy) = proxy {
            builder = builder.proxy(reqwest::Proxy::all(proxy.url())?);
        }
        builder.build()
    }

    /// Assemble a fetcher from explicit options, recording any client build
    /// error so the first fetch reports it instead of silently ignoring it.
    fn configured(
        user_agent: Option<String>,
        connect_timeout: Option<Duration>,
        read_timeout: Option<Duration>,
        proxy: Option<ProxyConfig>,
    ) -> Self {
        let (client, client_error) = match Self::build_client(
            user_agent.as_deref(),
            connect_timeout,
            read_timeout,
            proxy.as_ref(),
        ) {
            Ok(client) => (client, None),
            Err(error) => (reqwest::Client::new(), Some(error.to_string())),
        };
        Self {
            client,
            client_error,
            user_agent,
            connect_timeout,
            read_timeout,
            proxy,
            schemes: vec!["http".to_owned(), "https".to_owned()],
            retry: RetryPolicy::default(),
            resume: false,
            max_bytes: None,
            deny_link_local: false,
        }
    }

    /// Rebuild the client after an option change.
    fn rebuild_client(&mut self) {
        match Self::build_client(
            self.user_agent.as_deref(),
            self.connect_timeout,
            self.read_timeout,
            self.proxy.as_ref(),
        ) {
            Ok(client) => {
                self.client = client;
                self.client_error = None;
            }
            Err(error) => {
                self.client = reqwest::Client::new();
                self.client_error = Some(error.to_string());
            }
        }
    }

    /// The client, or the recorded build error (e.g. an invalid proxy URL).
    fn client(&self) -> Result<&reqwest::Client, FetchError> {
        match &self.client_error {
            Some(error) => Err(FetchError::Network(format!(
                "invalid HTTP client configuration: {error}"
            ))),
            None => Ok(&self.client),
        }
    }

    /// Override the `User-Agent` sent with requests.
    pub fn with_user_agent(user_agent: &str) -> Self {
        Self::configured(Some(user_agent.to_owned()), None, None, None)
    }

    /// Bound how long the client waits to establish a connection.
    ///
    /// Off by default (reqwest waits indefinitely). This is the TCP/TLS connect
    /// phase only; use [`HttpFetcher::with_read_timeout`] to bound a transfer
    /// that has connected but gone quiet.
    ///
    /// ```
    /// use ptox::HttpFetcher;
    /// use std::time::Duration;
    ///
    /// let fetcher = HttpFetcher::new().with_connect_timeout(Duration::from_secs(5));
    /// assert_eq!(fetcher.connect_timeout(), Some(Duration::from_secs(5)));
    /// ```
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = Some(timeout);
        self.rebuild_client();
        self
    }

    /// Bound how long the client waits for more bytes between reads.
    ///
    /// The timeout resets after every successful read, so it detects a stalled
    /// connection without capping a legitimately large or slow download (unlike
    /// a total request deadline). Off by default.
    ///
    /// ```
    /// use ptox::HttpFetcher;
    /// use std::time::Duration;
    ///
    /// let fetcher = HttpFetcher::new().with_read_timeout(Duration::from_secs(30));
    /// assert_eq!(fetcher.read_timeout(), Some(Duration::from_secs(30)));
    /// ```
    pub fn with_read_timeout(mut self, timeout: Duration) -> Self {
        self.read_timeout = Some(timeout);
        self.rebuild_client();
        self
    }

    /// The configured connect timeout, if any.
    pub fn connect_timeout(&self) -> Option<Duration> {
        self.connect_timeout
    }

    /// The configured read timeout, if any.
    pub fn read_timeout(&self) -> Option<Duration> {
        self.read_timeout
    }

    /// Route every request through `proxy`.
    ///
    /// Use `socks5h://` for overlay names (`.onion`, `.i2p`) so the proxy resolves
    /// the host, and `http://` for an HTTP proxy such as i2pd's. Register such a
    /// fetcher under the *overlay* scheme only, leaving the shared `http`/`https`
    /// entry unproxied.
    ///
    /// ```
    /// use ptox::HttpFetcher;
    ///
    /// let fetcher = HttpFetcher::new().with_proxy("socks5h://127.0.0.1:9050");
    /// assert_eq!(fetcher.proxy().map(|p| p.url()), Some("socks5h://127.0.0.1:9050"));
    /// ```
    pub fn with_proxy(mut self, proxy: impl Into<ProxyConfig>) -> Self {
        self.proxy = Some(proxy.into());
        self.rebuild_client();
        self
    }

    /// The configured proxy, if any.
    pub fn proxy(&self) -> Option<&ProxyConfig> {
        self.proxy.as_ref()
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

    /// Enable resuming an interrupted download from `<destination>.part`.
    ///
    /// Resuming is **off by default**. When enabled, a `.part` file left by an
    /// earlier attempt (or an earlier process) is re-hashed and continued with an
    /// HTTP `Range` request instead of restarting from zero, and a transient
    /// failure keeps the partial so the next attempt can pick it up. The final
    /// sha256 still covers the whole file, so verification is unaffected.
    ///
    /// A sidecar file (`.part.src`) records which address a partial came from, so
    /// a resume never splices bytes from two different mirrors. Because a resume
    /// trusts the saved prefix, it is most useful when the result carries a
    /// sha256: a corrupt or truncated partial is then rejected at verification.
    ///
    /// ```no_run
    /// use ptox::HttpFetcher;
    ///
    /// let fetcher = HttpFetcher::new().with_resume(true);
    /// assert!(fetcher.resume_enabled());
    /// ```
    pub fn with_resume(mut self, resume: bool) -> Self {
        self.resume = resume;
        self
    }

    /// Whether partial-download resuming is enabled.
    pub fn resume_enabled(&self) -> bool {
        self.resume
    }

    /// Cap the number of bytes a single fetch may write.
    ///
    /// Off by default (unbounded). When set, a response body that would exceed
    /// `max_bytes` aborts with [`FetchError::TooLarge`] and the partial is
    /// discarded — a guard against a malicious or misbehaving source exhausting
    /// disk. The cap is enforced on the bytes actually streamed, so a lying or
    /// absent `Content-Length` cannot get around it.
    ///
    /// ```
    /// use ptox::HttpFetcher;
    ///
    /// let fetcher = HttpFetcher::new().with_max_bytes(64 * 1024 * 1024);
    /// assert_eq!(fetcher.max_bytes(), Some(64 * 1024 * 1024));
    /// ```
    pub fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = Some(max_bytes);
        self
    }

    /// The configured download cap, if any.
    pub fn max_bytes(&self) -> Option<u64> {
        self.max_bytes
    }

    /// Refuse `http(s)` requests to a link-local or unspecified host — the SSRF
    /// path to cloud metadata (`169.254.169.254`) and the like.
    ///
    /// Off by default. This checks a **literal** IP host (and a few metadata
    /// hostnames); it cannot see an address that DNS resolves to a link-local IP,
    /// so pair it with a resolver policy if that matters. Loopback and private
    /// LAN hosts are allowed, since local gateways live there.
    ///
    /// ```
    /// use ptox::HttpFetcher;
    ///
    /// let fetcher = HttpFetcher::new().with_deny_link_local(true);
    /// assert!(fetcher.denies_link_local());
    /// ```
    pub fn with_deny_link_local(mut self, deny: bool) -> Self {
        self.deny_link_local = deny;
        self
    }

    /// Whether link-local hosts are refused.
    pub fn denies_link_local(&self) -> bool {
        self.deny_link_local
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
        if self.deny_link_local {
            if let Some(host) = host_of(&request.address.uri) {
                if is_link_local_host(host) {
                    return Err(FetchError::Denied(format!(
                        "`{host}` is a link-local/unspecified (SSRF) host"
                    )));
                }
            }
        }
        let temp = temp_path(&request.destination);
        let marker = resume_marker_path(&temp);
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

        let cleanup = || async {
            let _ = tokio::fs::remove_file(&temp).await;
            let _ = tokio::fs::remove_file(&marker).await;
        };

        let mut last_error = None;
        for attempt in 1..=attempts {
            match self.fetch_once(request, &monotonic).await {
                Ok(fetched) => return Ok(fetched),
                Err(AttemptError::Transient(error)) if attempt < attempts => {
                    last_error = Some(error);
                    // With resume on, keep the partial so the next attempt can
                    // continue it; otherwise start clean.
                    if !self.resume {
                        cleanup().await;
                    }
                    let delay = self.retry.delay_for(attempt);
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                }
                Err(AttemptError::Transient(error)) => {
                    if !self.resume {
                        cleanup().await;
                    }
                    return Err(error);
                }
                Err(AttemptError::Permanent(error)) => {
                    // A bad request, hash mismatch, or disk error cannot be fixed
                    // by resuming, so never leave the partial behind.
                    cleanup().await;
                    return Err(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| FetchError::Network("retry exhausted".to_owned())))
    }
}

impl HttpFetcher {
    /// Perform one HTTP GET + download, classifying failures as transient or
    /// permanent for the retry loop.
    ///
    /// When [`HttpFetcher::with_resume`] is on, an interrupted
    /// `<destination>.part` left by an earlier attempt (or an earlier process) is
    /// continued with a `Range` request instead of being restarted. Cleanup of
    /// the partial on failure is handled by [`Fetcher::fetch_with_progress`], so
    /// the retry loop can choose to keep it for a later resume.
    async fn fetch_once(
        &self,
        request: &FetchRequest,
        progress: &ProgressFn<'_>,
    ) -> Result<Fetched, AttemptError> {
        let temp = temp_path(&request.destination);
        let marker = resume_marker_path(&temp);

        // Decide the resume point before the request: the saved prefix must be
        // re-hashed to keep the final sha256 correct, and a prefix that cannot be
        // re-read means starting over from zero.
        let mut resume_from = if self.resume {
            resumable_prefix(&temp, &marker, &request.address.uri).await
        } else {
            0
        };
        let mut hasher = Sha256::new();
        if resume_from > 0 {
            match hash_prefix(&temp, resume_from, &mut hasher).await {
                Ok(()) => {}
                Err(_) => {
                    resume_from = 0;
                    hasher = Sha256::new();
                }
            }
        }
        if self.resume && resume_from == 0 {
            // A fresh partial: record the source now so a crash mid-download can
            // be resumed, and drop any stale partial from another address.
            let _ = tokio::fs::remove_file(&temp).await;
            tokio::fs::write(&marker, &request.address.uri)
                .await
                .map_err(|error| AttemptError::Permanent(FetchError::Io(error)))?;
        }

        let mut builder = self.client()?.get(&request.address.uri);
        if resume_from > 0 {
            builder = builder.header(reqwest::header::RANGE, format!("bytes={resume_from}-"));
        }
        let response = builder
            .send()
            .await
            .map_err(|error| AttemptError::Transient(FetchError::Network(error.to_string())))?;
        let status = response.status();

        // A complete partial: the server refuses the range because there is
        // nothing left to send. Verify what we already have and finish it.
        if resume_from > 0 && status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
            if let Some(total) = content_range_total(response.headers()) {
                if resume_from >= total {
                    return finish_partial(
                        request,
                        &temp,
                        &marker,
                        resume_from,
                        hasher,
                        self.max_bytes,
                    )
                    .await;
                }
            }
            return Err(AttemptError::Permanent(FetchError::Network(format!(
                "HTTP {status} for {}",
                request.address.uri
            ))));
        }

        if !status.is_success() {
            let error = FetchError::Network(format!("HTTP {status} for {}", request.address.uri));
            return Err(if is_transient_status(status) {
                AttemptError::Transient(error)
            } else {
                AttemptError::Permanent(error)
            });
        }

        // Append only when the server honored the exact range we asked for. A
        // `200` (range ignored, or no range requested) restarts from zero; a
        // mismatched `206` is rejected rather than silently truncating.
        let appending = resume_from > 0
            && status == reqwest::StatusCode::PARTIAL_CONTENT
            && content_range_start(response.headers()) == Some(resume_from);
        if resume_from > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT && !appending {
            return Err(AttemptError::Permanent(FetchError::Network(format!(
                "unexpected Content-Range for a range request on {}",
                request.address.uri
            ))));
        }
        let (hasher, total) = if appending {
            (hasher, resume_from)
        } else {
            (Sha256::new(), 0)
        };

        match download(
            response,
            &temp,
            request,
            progress,
            Resuming {
                hasher,
                total,
                append: appending,
            },
            self.max_bytes,
        )
        .await
        {
            Ok(fetched) => {
                if self.resume {
                    let _ = tokio::fs::remove_file(&marker).await;
                }
                Ok(fetched)
            }
            Err(error) => Err(match error {
                network @ FetchError::Network(_) => AttemptError::Transient(network),
                other => AttemptError::Permanent(other),
            }),
        }
    }
}

/// Verify and rename a partial the server reports as already complete.
async fn finish_partial(
    request: &FetchRequest,
    temp: &Path,
    marker: &Path,
    bytes: u64,
    hasher: Sha256,
    max_bytes: Option<u64>,
) -> Result<Fetched, AttemptError> {
    if let Some(limit) = max_bytes {
        if bytes > limit {
            return Err(AttemptError::Permanent(FetchError::TooLarge { limit }));
        }
    }
    let digest = hex::encode(hasher.finalize());
    verify_sha256(request.expected_sha256.as_deref(), &digest).map_err(AttemptError::Permanent)?;
    tokio::fs::rename(temp, &request.destination)
        .await
        .map_err(|error| AttemptError::Permanent(FetchError::Io(error)))?;
    let _ = tokio::fs::remove_file(marker).await;
    Ok(Fetched {
        path: request.destination.clone(),
        bytes,
        sha256: Some(digest),
        address: request.address.clone(),
    })
}

/// A download's resumable state: the running hash, the byte count so far, and
/// whether the body appends to an existing partial.
struct Resuming {
    hasher: Sha256,
    total: u64,
    append: bool,
}

/// Stream `response` to `temp`, verify its hash, then rename it into place.
///
/// `resume.hasher` and `resume.total` may carry a pre-hashed prefix when
/// resuming; `resume.append` selects appending to an existing `temp` instead of
/// truncating it.
async fn download(
    response: reqwest::Response,
    temp: &Path,
    request: &FetchRequest,
    progress: &ProgressFn<'_>,
    mut resume: Resuming,
    max_bytes: Option<u64>,
) -> Result<Fetched, FetchError> {
    if let Some(parent) = temp.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut file = if resume.append {
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(temp)
            .await?
    } else {
        tokio::fs::File::create(temp).await?
    };
    if resume.total > 0 {
        progress(resume.total);
    }
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| FetchError::Network(error.to_string()))?;
        if let Some(limit) = max_bytes {
            if resume.total + chunk.len() as u64 > limit {
                return Err(FetchError::TooLarge { limit });
            }
        }
        resume.hasher.update(&chunk);
        file.write_all(&chunk).await?;
        resume.total += chunk.len() as u64;
        progress(resume.total);
    }
    file.flush().await?;

    let digest = hex::encode(resume.hasher.finalize());
    verify_sha256(request.expected_sha256.as_deref(), &digest)?;
    tokio::fs::rename(temp, &request.destination).await?;

    Ok(Fetched {
        path: request.destination.clone(),
        bytes: resume.total,
        sha256: Some(digest),
        address: request.address.clone(),
    })
}

/// The host of a `scheme://[user@]host[:port]/...` URI (no `url` dependency).
fn host_of(uri: &str) -> Option<&str> {
    let rest = uri.split_once("://")?.1;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    if let Some(bracketed) = host_port.strip_prefix('[') {
        return bracketed.split(']').next();
    }
    Some(host_port.split(':').next().unwrap_or(host_port))
}

/// Whether `host` is a link-local / unspecified literal IP, or a known metadata
/// name. DNS-resolved link-local addresses are out of reach here.
fn is_link_local_host(host: &str) -> bool {
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return is_blocked_ip(ip);
    }
    matches!(
        host.to_ascii_lowercase().as_str(),
        "metadata.google.internal" | "metadata.goog" | "instance-data"
    )
}

fn is_blocked_ip(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => v4.is_link_local() || v4.is_unspecified() || v4.is_broadcast(),
        IpAddr::V6(v6) => {
            v6.is_unspecified()
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                || v6
                    .to_ipv4_mapped()
                    .is_some_and(|v4| is_blocked_ip(IpAddr::V4(v4)))
        }
    }
}

/// `<destination>.part.src`: records which address a partial download came from,
/// so a resume never splices bytes from two different mirrors.
fn resume_marker_path(temp: &Path) -> PathBuf {
    let mut name = temp.as_os_str().to_os_string();
    name.push(".src");
    PathBuf::from(name)
}

/// The number of bytes of `temp` that may be resumed, or `0` when there is no
/// partial matching `uri`.
async fn resumable_prefix(temp: &Path, marker: &Path, uri: &str) -> u64 {
    let Ok(recorded) = tokio::fs::read_to_string(marker).await else {
        return 0;
    };
    if recorded.trim() != uri {
        return 0;
    }
    match tokio::fs::metadata(temp).await {
        Ok(metadata) if metadata.is_file() => metadata.len(),
        _ => 0,
    }
}

/// Feed the first `len` bytes of `path` into `hasher` so a resumed download's
/// final digest covers the whole file.
async fn hash_prefix(path: &Path, len: u64, hasher: &mut Sha256) -> std::io::Result<()> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut remaining = len;
    let mut buffer = vec![0u8; 64 * 1024];
    while remaining > 0 {
        let want = remaining.min(buffer.len() as u64) as usize;
        let read = file.read(&mut buffer[..want]).await?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "partial file shrank while resuming",
            ));
        }
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
    }
    Ok(())
}

/// The start offset of a `Content-Range: bytes <start>-<end>/<total>` header.
fn content_range_start(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let value = headers.get(reqwest::header::CONTENT_RANGE)?.to_str().ok()?;
    let rest = value.trim().strip_prefix("bytes")?.trim_start();
    let (range, _total) = rest.split_once('/')?;
    let (start, _end) = range.split_once('-')?;
    start.trim().parse().ok()
}

/// The total size in a `Content-Range` header (a `416` sends `bytes */<total>`).
fn content_range_total(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let value = headers.get(reqwest::header::CONTENT_RANGE)?.to_str().ok()?;
    let (_range, total) = value.trim().rsplit_once('/')?;
    total.trim().parse().ok()
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

    #[test]
    fn resume_is_off_by_default() {
        assert!(!HttpFetcher::new().resume_enabled());
        assert!(HttpFetcher::new().with_resume(true).resume_enabled());
    }

    #[test]
    fn timeouts_are_off_by_default_and_chain() {
        let fetcher = HttpFetcher::new();
        assert_eq!(fetcher.connect_timeout(), None);
        assert_eq!(fetcher.read_timeout(), None);

        let fetcher = HttpFetcher::new()
            .with_connect_timeout(Duration::from_secs(3))
            .with_read_timeout(Duration::from_secs(17));
        assert_eq!(fetcher.connect_timeout(), Some(Duration::from_secs(3)));
        assert_eq!(fetcher.read_timeout(), Some(Duration::from_secs(17)));
        // User-Agent changes survive later timeout changes (the client is rebuilt).
        assert_eq!(fetcher.user_agent.as_deref(), Some(DEFAULT_USER_AGENT));
    }

    #[test]
    fn parses_content_range_start_and_total() {
        use reqwest::header::{HeaderMap, HeaderValue, CONTENT_RANGE};
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_RANGE, HeaderValue::from_static("bytes 5-99/1000"));
        assert_eq!(content_range_start(&headers), Some(5));
        assert_eq!(content_range_total(&headers), Some(1000));

        headers.insert(CONTENT_RANGE, HeaderValue::from_static("bytes */1000"));
        assert_eq!(content_range_start(&headers), None);
        assert_eq!(content_range_total(&headers), Some(1000));

        headers.insert(CONTENT_RANGE, HeaderValue::from_static("nonsense"));
        assert_eq!(content_range_start(&headers), None);
        assert_eq!(content_range_total(&headers), None);
    }

    #[tokio::test]
    async fn resume_marker_must_match_a_readable_partial() {
        let dir = std::env::temp_dir().join(format!("ptox-resume-unit-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.expect("mkdir");
        let temp = dir.join("file.part");
        let marker = resume_marker_path(&temp);
        assert_eq!(marker, dir.join("file.part.src"));

        // No marker yet.
        tokio::fs::write(&temp, b"hello").await.expect("write");
        assert_eq!(resumable_prefix(&temp, &marker, "https://a/x").await, 0);

        // Matching source and partial.
        tokio::fs::write(&marker, "https://a/x")
            .await
            .expect("marker");
        assert_eq!(resumable_prefix(&temp, &marker, "https://a/x").await, 5);

        // A different source must not resume another mirror's bytes.
        assert_eq!(resumable_prefix(&temp, &marker, "https://b/x").await, 0);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
