//! Retrieve `ipfs://` content through an HTTP gateway.
//!
//! IPFS content is addressed by content identifier (CID), not by host, so an
//! `ipfs://` address carries no way to dial it directly. The pragmatic bridge is
//! an **HTTP gateway**: rewrite `ipfs://<cid>[/path]` to
//! `<gateway>/ipfs/<cid>[/path]` and fetch that with the ordinary
//! [`HttpFetcher`]. This module is that rewrite.
//!
//! ```no_run
//! # async fn run() -> Result<(), ptox::FetchError> {
//! use ptox::{Address, FetchRegistry, IpfsGatewayFetcher};
//! use std::sync::Arc;
//!
//! let mut registry = FetchRegistry::new();
//! registry.register(Arc::new(IpfsGatewayFetcher::new("http://127.0.0.1:8080")));
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
//! [`IpfsGatewayFetcher::new`]. The rewrite does not verify that the gateway
//! returns the content matching the CID; integrity comes from the content hash
//! the caller passes in `expected_sha256` (which [`super::FetchRegistry`] fills
//! from a merged result's `sha256` when present).

use super::{FetchError, FetchRequest, Fetched, Fetcher, HttpFetcher, ProgressFn};
use crate::types::Address;
use async_trait::async_trait;

/// The public gateway used by [`IpfsGatewayFetcher::with_default_gateway`].
///
/// Suitable for examples and tests; production clients should choose their own.
pub const DEFAULT_IPFS_GATEWAY: &str = "https://ipfs.io";

/// The scheme this fetcher resolves.
const IPFS_SCHEME: &str = "ipfs";

/// Resolves `ipfs://<cid>[/path]` by rewriting it onto an HTTP gateway and
/// delegating to [`HttpFetcher`].
///
/// Progress reporting and atomic writes are inherited from [`HttpFetcher`], so
/// the fetcher works identically from [`Fetcher::fetch`] and
/// [`Fetcher::fetch_with_progress`].
pub struct IpfsGatewayFetcher {
    gateway: String,
    http: HttpFetcher,
    schemes: Vec<String>,
}

impl IpfsGatewayFetcher {
    /// A fetcher that resolves IPFS addresses through `gateway` (e.g.
    /// `https://ipfs.io` or `http://127.0.0.1:8080`). A trailing `/` is ignored.
    pub fn new(gateway: impl Into<String>) -> Self {
        Self {
            gateway: gateway.into().trim_end_matches('/').to_owned(),
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
    /// the gateway request instead of the default [`HttpFetcher`].
    pub fn with_http_fetcher(mut self, http: HttpFetcher) -> Self {
        self.http = http;
        self
    }

    /// The gateway base URL this fetcher resolves through.
    pub fn gateway(&self) -> &str {
        &self.gateway
    }

    /// Rewrite an `ipfs://` URI into the equivalent gateway URL.
    ///
    /// The CID and any path/query/fragment are forwarded verbatim; only the
    /// `ipfs://` prefix is replaced with `<gateway>/ipfs/`.
    pub fn resolve(&self, uri: &str) -> Result<String, FetchError> {
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
        Ok(format!("{}/ipfs/{rest}", self.gateway))
    }

    /// Rewrite `request` onto the gateway, preserving destination and hash.
    fn rewrite(&self, request: &FetchRequest) -> Result<FetchRequest, FetchError> {
        Ok(FetchRequest {
            address: Address::new("https", self.resolve(&request.address.uri)?),
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
        self.http.fetch(&self.rewrite(request)?).await
    }

    async fn fetch_with_progress(
        &self,
        request: &FetchRequest,
        progress: &ProgressFn<'_>,
    ) -> Result<Fetched, FetchError> {
        self.http
            .fetch_with_progress(&self.rewrite(request)?, progress)
            .await
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
