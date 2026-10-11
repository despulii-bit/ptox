//! Retrieve `<scheme>://…` addresses through an HTTP gateway.
//!
//! Some networks address content by *value* (a content id, a key) rather than by
//! host, so a `<scheme>://<rest>` address carries no way to dial it directly. The
//! pragmatic bridge is an **HTTP gateway**: rewrite `<scheme>://<rest>` to
//! `<gateway><prefix><rest>` and fetch that with the ordinary [`HttpFetcher`].
//!
//! This is the generic form of [`crate::IpfsGatewayFetcher`]: IPFS, FProxy, and
//! other gateway services all reduce to a gateway base plus a path prefix. A
//! fetcher holds one or more gateways and tries them in order, failing over when
//! one is unreachable; the winning gateway is reported on [`Fetched::address`].
//!
//! ```no_run
//! # async fn run() -> Result<(), ptox::FetchError> {
//! use ptox::{Address, FetchRegistry, GatewayFetcher};
//! use std::sync::Arc;
//!
//! let mut registry = FetchRegistry::new();
//! // `freenet://<key>` -> `http://127.0.0.1:8888/<key>`
//! registry.register(Arc::new(GatewayFetcher::new(
//!     "freenet",
//!     "/",
//!     ["http://127.0.0.1:8888"],
//! )));
//! let address = Address::new("freenet", "freenet://CHK@abc");
//! let fetched = registry.fetch(&address, "file", None).await?;
//! # let _ = fetched; Ok(())
//! # }
//! ```
//!
//! # Gateway policy
//!
//! A gateway sees every request, so which gateway you use is a privacy and
//! availability decision: pass your own (self-hosted, or a setting) rather than
//! trusting a public one. The rewrite does not verify that the gateway returns
//! content matching the address; integrity comes from the content hash the caller
//! passes in `expected_sha256` (which [`super::FetchRegistry`] fills from a merged
//! result's `sha256` when present).

use super::{FetchError, FetchRequest, Fetched, Fetcher, HttpFetcher, ProgressFn};
use crate::types::Address;
use async_trait::async_trait;
use std::sync::atomic::{AtomicU64, Ordering};

/// Resolves `<scheme>://<rest>` by rewriting it onto an HTTP gateway and
/// delegating to [`HttpFetcher`].
///
/// The address becomes `<gateway><prefix><rest>`; `prefix` defaults to `/` but is
/// `/ipfs/` for IPFS, and so on. Holds one or more gateways, tried in order.
pub struct GatewayFetcher {
    scheme: String,
    prefix: String,
    gateways: Vec<String>,
    http: HttpFetcher,
    schemes: Vec<String>,
}

impl GatewayFetcher {
    /// A fetcher for `scheme` addresses through `gateways`, rewriting
    /// `<scheme>://<rest>` to `<gateway><prefix><rest>`.
    ///
    /// `prefix` is normalized to a leading and trailing `/` (so `"ipfs"` and
    /// `"/ipfs/"` are the same). A trailing `/` on a gateway is ignored.
    pub fn new<I, S>(scheme: impl Into<String>, prefix: impl Into<String>, gateways: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let scheme = scheme.into();
        let gateways = gateways
            .into_iter()
            .map(Into::into)
            .map(|gateway: String| gateway.trim_end_matches('/').to_owned())
            .collect();
        Self {
            schemes: vec![scheme.clone()],
            scheme,
            prefix: normalize_prefix(prefix.into()),
            gateways,
            http: HttpFetcher::new(),
        }
    }

    /// Use `http` (custom `User-Agent`, proxy, timeouts, a byte cap, …) for the
    /// gateway requests instead of the default [`HttpFetcher`].
    pub fn with_http_fetcher(mut self, http: HttpFetcher) -> Self {
        self.http = http;
        self
    }

    /// The scheme this fetcher resolves.
    pub fn scheme(&self) -> &str {
        &self.scheme
    }

    /// The normalized path prefix inserted before `<rest>`.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// The preferred (first) gateway, or `""` when none is configured.
    pub fn gateway(&self) -> &str {
        self.gateways.first().map(String::as_str).unwrap_or("")
    }

    /// Every configured gateway, in preference order.
    pub fn gateways(&self) -> &[String] {
        &self.gateways
    }

    /// Rewrite a `<scheme>://` URI onto the preferred gateway.
    pub fn resolve(&self, uri: &str) -> Result<String, FetchError> {
        Self::resolve_with(&self.scheme, &self.prefix, self.gateway(), uri)
    }

    fn resolve_with(
        scheme: &str,
        prefix: &str,
        gateway: &str,
        uri: &str,
    ) -> Result<String, FetchError> {
        let marker = format!("{scheme}://");
        let rest = uri
            .strip_prefix(&marker)
            .ok_or_else(|| FetchError::InvalidAddress(format!("not a {scheme} URI: `{uri}`")))?;
        // The authority is a value, not a host, so tolerate a leading `/`.
        let rest = rest.trim_start_matches('/');
        if rest.is_empty() {
            return Err(FetchError::InvalidAddress(format!(
                "{scheme} URI has nothing after the scheme: `{uri}`"
            )));
        }
        Ok(format!("{gateway}{prefix}{rest}"))
    }

    fn rewrite(
        scheme: &str,
        prefix: &str,
        gateway: &str,
        request: &FetchRequest,
    ) -> Result<FetchRequest, FetchError> {
        let resolved = Self::resolve_with(scheme, prefix, gateway, &request.address.uri)?;
        let scheme = if resolved.starts_with("http://") {
            "http"
        } else {
            "https"
        };
        Ok(FetchRequest {
            address: Address::new(scheme, resolved),
            destination: request.destination.clone(),
            expected_sha256: request.expected_sha256.clone(),
        })
    }
}

/// A leading and trailing `/`, so `"ipfs"` and `"/ipfs/"` both become `"/ipfs/"`.
fn normalize_prefix(prefix: String) -> String {
    let trimmed = prefix.trim().trim_matches('/');
    if trimmed.is_empty() {
        "/".to_owned()
    } else {
        format!("/{trimmed}/")
    }
}

#[async_trait]
impl Fetcher for GatewayFetcher {
    fn name(&self) -> &str {
        "gateway"
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
        if self.gateways.is_empty() {
            return Err(FetchError::InvalidAddress(format!(
                "no {} gateway configured",
                self.scheme
            )));
        }

        // A malformed URI fails identically for every gateway; resolve once so it
        // is reported directly rather than as an `AllFailed`. Progress stays
        // monotonic across a failed gateway, so the bar never jumps backwards.
        let high_water = AtomicU64::new(0);
        let mut errors = Vec::new();
        for gateway in &self.gateways {
            let rewritten = Self::rewrite(&self.scheme, &self.prefix, gateway, request)?;
            let monotonic = |bytes: u64| {
                let previous = high_water.fetch_max(bytes, Ordering::Relaxed);
                if bytes > previous {
                    progress(bytes);
                }
            };
            match self.http.fetch_with_progress(&rewritten, &monotonic).await {
                // `Fetched::address` is the rewritten gateway URL the bytes came
                // from, so the winning gateway is reported.
                Ok(fetched) => return Ok(fetched),
                Err(error) => errors.push(error),
            }
        }

        let count = errors.len();
        Err(FetchError::AllFailed { count, errors })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_with_a_root_prefix() {
        let fetcher = GatewayFetcher::new("freenet", "/", ["http://127.0.0.1:8888"]);
        assert_eq!(
            fetcher.resolve("freenet://CHK@abc").expect("resolve"),
            "http://127.0.0.1:8888/CHK@abc"
        );
        assert_eq!(
            fetcher
                .resolve("freenet:///USK@abc/site/1/x")
                .expect("resolve"),
            "http://127.0.0.1:8888/USK@abc/site/1/x"
        );
    }

    #[test]
    fn resolves_with_a_path_prefix_like_ipfs() {
        let fetcher = GatewayFetcher::new("ipfs", "ipfs", ["https://ipfs.io/"]);
        assert_eq!(fetcher.prefix(), "/ipfs/");
        assert_eq!(
            fetcher
                .resolve("ipfs://bafyabc/docs/x.pdf?name=x")
                .expect("resolve"),
            "https://ipfs.io/ipfs/bafyabc/docs/x.pdf?name=x"
        );
        assert_eq!(fetcher.gateway(), "https://ipfs.io");
    }

    #[test]
    fn keeps_gateways_in_preference_order() {
        let fetcher = GatewayFetcher::new(
            "freenet",
            "/",
            ["http://127.0.0.1:8888/", "https://fproxy.example"],
        );
        assert_eq!(
            fetcher.gateways(),
            ["http://127.0.0.1:8888", "https://fproxy.example"]
        );
    }

    #[test]
    fn rejects_a_foreign_scheme_and_an_empty_address() {
        let fetcher = GatewayFetcher::new("freenet", "/", ["http://127.0.0.1:8888"]);
        assert!(fetcher.resolve("ipfs://bafyabc").is_err());
        assert!(fetcher.resolve("freenet://").is_err());
    }

    #[tokio::test]
    async fn empty_gateway_list_is_an_error() {
        let fetcher = GatewayFetcher::new("freenet", "/", Vec::<String>::new());
        let request = FetchRequest {
            address: Address::new("freenet", "freenet://CHK@abc"),
            destination: std::path::PathBuf::from("out"),
            expected_sha256: None,
        };
        let error = fetcher.fetch(&request).await.expect_err("no gateway");
        assert!(
            matches!(error, FetchError::InvalidAddress(_)),
            "got {error:?}"
        );
    }
}
