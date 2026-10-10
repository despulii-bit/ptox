//! Retrieve `ipfs://` content through an HTTP gateway.
//!
//! IPFS content is addressed by content identifier (CID), not by host, so an
//! `ipfs://` address carries no way to dial it directly. The pragmatic bridge is
//! an **HTTP gateway**: rewrite `ipfs://<cid>[/path]` to
//! `<gateway>/ipfs/<cid>[/path]` and fetch that with the ordinary
//! [`HttpFetcher`]. This module is that rewrite.
//!
//! A fetcher can hold several gateways and tries them in order, failing over
//! when one is unreachable; the winning gateway is reported on
//! [`Fetched::address`].
//!
//! ```no_run
//! # async fn run() -> Result<(), ptox::FetchError> {
//! use ptox::{Address, FetchRegistry, IpfsGatewayFetcher};
//! use std::sync::Arc;
//!
//! let mut registry = FetchRegistry::new();
//! registry.register(Arc::new(IpfsGatewayFetcher::with_gateways([
//!     "http://127.0.0.1:8080",
//!     "https://ipfs.io",
//! ])));
//! let address = Address::new("ipfs", "ipfs://bafy.../report.pdf");
//! let fetched = registry.fetch(&address, "report.pdf", None).await?;
//! println!("{} bytes -> {}", fetched.bytes, fetched.path.display());
//! # Ok(())
//! # }
//! ```
//!
//! # Gateway policy
//!
//! A gateway sees every request, so which gateway you use is a privacy and
//! availability decision, not an implementation detail. [`DEFAULT_IPFS_GATEWAY`]
//! is a public convenience for examples and tests; a real client should pass its
//! own gateway (self-hosted, a trusted provider, or a configurable setting) to
//! [`IpfsGatewayFetcher::new`] or [`IpfsGatewayFetcher::with_gateways`]. The
//! rewrite does not verify that the gateway returns the content matching the
//! CID; integrity comes from the content hash the caller passes in
//! `expected_sha256` (which [`super::FetchRegistry`] fills from a merged result's
//! `sha256` when present).

use super::{FetchError, FetchRequest, Fetched, Fetcher, HttpFetcher, ProgressFn};
use crate::types::Address;
use async_trait::async_trait;
use std::sync::atomic::{AtomicU64, Ordering};

/// The public gateway used by [`IpfsGatewayFetcher::with_default_gateway`].
///
/// Suitable for examples and tests; production clients should choose their own.
pub const DEFAULT_IPFS_GATEWAY: &str = "https://ipfs.io";

/// The scheme this fetcher resolves.
const IPFS_SCHEME: &str = "ipfs";

/// Resolves `ipfs://<cid>[/path]` by rewriting it onto an HTTP gateway and
/// delegating to [`HttpFetcher`].
///
/// Holds one or more gateways; [`Fetcher::fetch`] tries them in order and
/// returns the first success. Progress reporting and atomic writes are inherited
/// from [`HttpFetcher`], so the fetcher works identically from
/// [`Fetcher::fetch`] and [`Fetcher::fetch_with_progress`].
pub struct IpfsGatewayFetcher {
    gateways: Vec<String>,
    http: HttpFetcher,
    schemes: Vec<String>,
}

impl IpfsGatewayFetcher {
    /// A fetcher that resolves IPFS addresses through `gateway` (e.g.
    /// `https://ipfs.io` or `http://127.0.0.1:8080`). A trailing `/` is ignored.
    pub fn new(gateway: impl Into<String>) -> Self {
        Self::with_gateways([gateway.into()])
    }

    /// A fetcher that tries `gateways` in order, failing over to the next when
    /// one is unreachable. The first entry is the preferred gateway. A trailing
    /// `/` on a gateway is ignored.
    ///
    /// [`Fetcher::fetch`] returns the first gateway that succeeds and records
    /// it on [`Fetched::address`]; if every gateway fails it returns
    /// [`FetchError::AllFailed`] carrying each attempt's error.
    pub fn with_gateways<I, S>(gateways: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let gateways = gateways
            .into_iter()
            .map(Into::into)
            .map(|gateway: String| gateway.trim_end_matches('/').to_owned())
            .collect();
        Self {
            gateways,
            http: HttpFetcher::new(),
            schemes: vec![IPFS_SCHEME.to_owned()],
        }
    }

    /// A fetcher using [`DEFAULT_IPFS_GATEWAY`].
    ///
    /// Convenient for examples and tests; prefer [`IpfsGatewayFetcher::new`] with
    /// a gateway you trust in production.
    pub fn with_default_gateway() -> Self {
        Self::new(DEFAULT_IPFS_GATEWAY)
    }

    /// Use `http` (e.g. a custom `User-Agent`, proxy, or TLS config) to perform
    /// the gateway requests instead of the default [`HttpFetcher`].
    pub fn with_http_fetcher(mut self, http: HttpFetcher) -> Self {
        self.http = http;
        self
    }

    /// The preferred (first) gateway, or `""` when none is configured.
    pub fn gateway(&self) -> &str {
        self.gateways.first().map(String::as_str).unwrap_or("")
    }

    /// Every configured gateway, in preference order.
    pub fn gateways(&self) -> &[String] {
        &self.gateways
    }

    /// Rewrite an `ipfs://` URI onto the preferred gateway.
    ///
    /// The CID and any path/query/fragment are forwarded verbatim; only the
    /// `ipfs://` prefix is replaced with `<gateway>/ipfs/`.
    pub fn resolve(&self, uri: &str) -> Result<String, FetchError> {
        Self::resolve_with(self.gateway(), uri)
    }

    fn resolve_with(gateway: &str, uri: &str) -> Result<String, FetchError> {
        let rest = uri
            .strip_prefix("ipfs://")
            .ok_or_else(|| FetchError::InvalidAddress(format!("not an ipfs URI: `{uri}`")))?;
        // `ipfs://<cid>/...`: the authority is the CID, so tolerate a leading `/`
        // (as in `ipfs:///bafy...`) rather than producing a double slash.
        let rest = rest.trim_start_matches('/');
        if rest.is_empty() {
            return Err(FetchError::InvalidAddress(format!(
                "ipfs URI has no content identifier: `{uri}`"
            )));
        }
        Ok(format!("{gateway}/ipfs/{rest}"))
    }

    /// Rewrite `request` onto `gateway`, preserving destination and hash.
    fn rewrite(gateway: &str, request: &FetchRequest) -> Result<FetchRequest, FetchError> {
        let resolved = Self::resolve_with(gateway, &request.address.uri)?;
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

#[async_trait]
impl Fetcher for IpfsGatewayFetcher {
    fn name(&self) -> &str {
        "ipfs-gateway"
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
            return Err(FetchError::InvalidAddress(
                "no ipfs gateway configured".to_owned(),
            ));
        }

        // A malformed URI fails identically for every gateway; resolve once so
        // that is reported directly rather than as an `AllFailed`.
        // Progress stays monotonic across a failed gateway: a partial download
        // from one gateway never makes the bar jump backwards on the next.
        let high_water = AtomicU64::new(0);
        let mut errors = Vec::new();
        for gateway in &self.gateways {
            let rewritten = Self::rewrite(gateway, request)?;
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
    fn resolves_cid_without_path() {
        let fetcher = IpfsGatewayFetcher::new("https://ipfs.io");
        assert_eq!(
            fetcher.resolve("ipfs://bafyabc").expect("resolve"),
            "https://ipfs.io/ipfs/bafyabc"
        );
    }

    #[test]
    fn resolves_cid_with_path_and_query() {
        let fetcher = IpfsGatewayFetcher::new("https://ipfs.io");
        assert_eq!(
            fetcher
                .resolve("ipfs://bafyabc/docs/report.pdf?filename=report.pdf")
                .expect("resolve"),
            "https://ipfs.io/ipfs/bafyabc/docs/report.pdf?filename=report.pdf"
        );
    }

    #[test]
    fn tolerates_trailing_gateway_slash_and_leading_cid_slash() {
        let fetcher = IpfsGatewayFetcher::new("https://ipfs.io/");
        assert_eq!(
            fetcher.resolve("ipfs:///bafyabc").expect("resolve"),
            "https://ipfs.io/ipfs/bafyabc"
        );
        assert_eq!(fetcher.gateway(), "https://ipfs.io");
    }

    #[test]
    fn keeps_gateways_in_preference_order() {
        let fetcher =
            IpfsGatewayFetcher::with_gateways(["http://127.0.0.1:8080/", "https://ipfs.io"]);
        assert_eq!(
            fetcher.gateways(),
            ["http://127.0.0.1:8080", "https://ipfs.io"]
        );
        assert_eq!(fetcher.gateway(), "http://127.0.0.1:8080");
    }

    #[test]
    fn rejects_wrong_scheme_and_empty_cid() {
        let fetcher = IpfsGatewayFetcher::new("https://ipfs.io");
        assert!(matches!(
            fetcher.resolve("https://example.org/file"),
            Err(FetchError::InvalidAddress(_))
        ));
        assert!(matches!(
            fetcher.resolve("ipfs://"),
            Err(FetchError::InvalidAddress(_))
        ));
        assert!(matches!(
            fetcher.resolve("ipfs:///"),
            Err(FetchError::InvalidAddress(_))
        ));
    }
}
