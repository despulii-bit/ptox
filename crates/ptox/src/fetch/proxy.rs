//! Retrieve `<scheme>://<host>[/path]` addresses on a proxy-based overlay
//! network (I2P, Tor, …).
//!
//! Overlay networks are not dialed directly; a local daemon provides a proxy
//! (i2pd's HTTP proxy, Tor's SOCKS port). [`ProxyFetcher`] rewrites
//! `<scheme>://<host>[/path]` to `http://<host>[/path]` and fetches it through
//! an [`HttpFetcher`] configured with a [`ProxyConfig`], so overlay retrieval
//! gets every HTTP guarantee — atomic writes, sha256 verification, progress,
//! retries, and timeouts — for free.
//!
//! ```no_run
//! # async fn run() -> Result<(), ptox::FetchError> {
//! use ptox::{Address, FetchRegistry, ProxyFetcher};
//! use std::sync::Arc;
//!
//! let mut registry = FetchRegistry::new();
//! registry.register(Arc::new(ProxyFetcher::tor()));  // socks5h://127.0.0.1:9050
//! registry.register(Arc::new(ProxyFetcher::i2p()));  // http://127.0.0.1:4444
//!
//! let address = Address::new("tor", "tor://exampleonion.onion/index.html");
//! let fetched = registry.fetch(&address, "index.html", None).await?;
//! println!("{} bytes -> {}", fetched.bytes, fetched.path.display());
//! # Ok(())
//! # }
//! ```

use super::{FetchError, FetchRequest, Fetched, Fetcher, HttpFetcher, ProgressFn, ProxyConfig};
use crate::types::Address;
use async_trait::async_trait;

/// The default i2pd HTTP proxy address.
pub const DEFAULT_I2P_PROXY: &str = "http://127.0.0.1:4444";

/// The default Tor SOCKS proxy address (remote DNS, so `.onion` names resolve).
pub const DEFAULT_TOR_PROXY: &str = "socks5h://127.0.0.1:9050";

/// Retrieves a proxy-based overlay scheme through a local proxy.
///
/// The address is `<scheme>://<host>[/path]` (e.g. `i2p://<b32>.i2p`,
/// `tor://<onion>/file`); the fetcher rewrites it to `http://<host>[/path]` and
/// delegates to an [`HttpFetcher`] bound to the proxy. HTTPS-only sites work via
/// the HTTP redirect the proxy follows.
///
/// ```no_run
/// use ptox::ProxyFetcher;
///
/// // A custom proxy and scheme:
/// let fetcher = ProxyFetcher::new("i2p", "http://127.0.0.1:8080");
/// assert_eq!(fetcher.scheme(), "i2p");
/// ```
pub struct ProxyFetcher {
    scheme: String,
    name: String,
    http: HttpFetcher,
    schemes: Vec<String>,
}

impl ProxyFetcher {
    /// A fetcher for `scheme` that routes through `proxy`.
    pub fn new(scheme: impl Into<String>, proxy: impl Into<ProxyConfig>) -> Self {
        let scheme = scheme.into();
        Self {
            name: format!("{scheme}-proxy"),
            schemes: vec![scheme.clone()],
            scheme,
            http: HttpFetcher::new().with_proxy(proxy),
        }
    }

    /// A fetcher for I2P through [`DEFAULT_I2P_PROXY`].
    pub fn i2p() -> Self {
        Self::new("i2p", DEFAULT_I2P_PROXY)
    }

    /// A fetcher for Tor through [`DEFAULT_TOR_PROXY`].
    pub fn tor() -> Self {
        Self::new("tor", DEFAULT_TOR_PROXY)
    }

    /// Use `http` to perform the request (e.g. to set timeouts, retries, or a
    /// custom `User-Agent`) instead of the default proxy-bound [`HttpFetcher`].
    ///
    /// The supplied fetcher should itself carry the proxy; this only replaces it.
    pub fn with_http_fetcher(mut self, http: HttpFetcher) -> Self {
        self.http = http;
        self
    }

    /// The overlay scheme this fetcher resolves.
    pub fn scheme(&self) -> &str {
        &self.scheme
    }

    /// The proxy in effect, if any.
    pub fn proxy(&self) -> Option<&ProxyConfig> {
        self.http.proxy()
    }

    /// Rewrite `<scheme>://<host>[/path]` to `http://<host>[/path]`.
    fn rewrite(&self, request: &FetchRequest) -> Result<FetchRequest, FetchError> {
        let prefix = format!("{}://", self.scheme);
        let rest = request
            .address
            .uri
            .strip_prefix(&prefix)
            .ok_or_else(|| {
                FetchError::InvalidAddress(format!(
                    "not a {} address: `{}`",
                    self.scheme, request.address.uri
                ))
            })?
            .trim_start_matches('/');
        if rest.is_empty() {
            return Err(FetchError::InvalidAddress(format!(
                "{} address has no host: `{}`",
                self.scheme, request.address.uri
            )));
        }
        Ok(FetchRequest {
            address: Address::new("http", format!("http://{rest}")),
            destination: request.destination.clone(),
            expected_sha256: request.expected_sha256.clone(),
        })
    }
}

#[async_trait]
impl Fetcher for ProxyFetcher {
    fn name(&self) -> &str {
        &self.name
    }

    fn schemes(&self) -> &[String] {
        &self.schemes
    }

    async fn fetch(&self, request: &FetchRequest) -> Result<Fetched, FetchError> {
        let mut fetched = self.http.fetch(&self.rewrite(request)?).await?;
        // Report the overlay address the caller asked for, not the proxy URL.
        fetched.address = request.address.clone();
        Ok(fetched)
    }

    async fn fetch_with_progress(
        &self,
        request: &FetchRequest,
        progress: &ProgressFn<'_>,
    ) -> Result<Fetched, FetchError> {
        let mut fetched = self
            .http
            .fetch_with_progress(&self.rewrite(request)?, progress)
            .await?;
        fetched.address = request.address.clone();
        Ok(fetched)
    }
}
