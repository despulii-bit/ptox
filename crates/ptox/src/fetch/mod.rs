//! The **retrieval half** of ptox: turn a [`crate::types::Address`] into bytes on disk.
//!
//! The search fabric emits `Address`es; this fabric consumes them. Fetchers are
//! registered per scheme, so retrieval is pluggable exactly like search.
//!
//! ```no_run
//! # async fn run() -> Result<(), ptox::FetchError> {
//! use ptox::Address;
//! let registry = ptox::FetchRegistry::with_defaults();
//! let address = Address::new("https", "https://example.org/file.bin");
//! let fetched = registry.fetch(&address, "file.bin", None).await?;
//! println!("{} bytes -> {}", fetched.bytes, fetched.path.display());
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]

mod command;
#[cfg(feature = "file")]
mod file;
mod http;
#[cfg(feature = "ipfs")]
mod ipfs;

pub use command::CommandFetcher;
#[cfg(feature = "file")]
pub use file::FileFetcher;
pub use http::{HttpFetcher, RetryPolicy};
#[cfg(feature = "ipfs")]
pub use ipfs::{IpfsGatewayFetcher, DEFAULT_IPFS_GATEWAY};

use crate::merge::MergedResult;
use crate::types::Address;
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

/// Errors that can occur while retrieving an address.
#[derive(Debug, Error)]
pub enum FetchError {
    /// No fetcher is registered for the address scheme.
    #[error("unsupported address scheme `{0}`")]
    Unsupported(String),
    /// Filesystem error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// Network or protocol error.
    #[error("network error: {0}")]
    Network(String),
    /// A helper command exited non-zero.
    #[error("command failed: {0}")]
    Command(String),
    /// Retrieved bytes did not match the expected hash.
    #[error("hash mismatch: expected {expected}, got {actual}")]
    HashMismatch {
        /// The expected digest (lowercase hex).
        expected: String,
        /// The digest actually retrieved (lowercase hex).
        actual: String,
    },
    /// The result carried no address to fetch.
    #[error("result has no address")]
    NoAddress,
    /// The address was malformed for its scheme (e.g. an `ipfs://` URI with no
    /// content identifier).
    #[error("invalid address: {0}")]
    InvalidAddress(String),
    /// Every address of a result failed to retrieve.
    #[error("all {count} address(es) failed")]
    AllFailed {
        /// How many addresses were tried.
        count: usize,
        /// The individual failures, in the order the addresses were tried.
        errors: Vec<FetchError>,
    },
    /// A local-file fetch was refused by policy (outside every configured root).
    #[error("access denied: {0}")]
    AccessDenied(String),
}

/// A single retrieval job: one address, one destination path.
#[derive(Debug, Clone)]
pub struct FetchRequest {
    /// The address to retrieve.
    pub address: Address,
    /// Where the bytes should be written.
    pub destination: PathBuf,
    /// Expected sha256 (lowercase hex); verified after retrieval.
    pub expected_sha256: Option<String>,
}

/// The outcome of a successful retrieval.
#[derive(Debug, Clone)]
pub struct Fetched {
    /// Path the bytes were written to.
    pub path: PathBuf,
    /// Number of bytes written.
    pub bytes: u64,
    /// sha256 of the retrieved bytes (lowercase hex), when computed.
    pub sha256: Option<String>,
    /// The address the bytes were actually retrieved from.
    ///
    /// With multi-address failover ([`FetchRegistry::fetch_merged`]) this is the
    /// first address that succeeded, which may not have been the preferred one.
    pub address: Address,
}

/// A progress observer, called with the cumulative number of bytes retrieved so
/// far. It is invoked once per received chunk, on the retrieval task, so it must
/// be cheap and must not block.
pub type ProgressFn<'a> = dyn Fn(u64) + Send + Sync + 'a;

/// One way of turning an address into bytes (one protocol).
#[async_trait]
pub trait Fetcher: Send + Sync {
    /// A stable name for diagnostics.
    fn name(&self) -> &str;
    /// The address schemes this fetcher handles.
    fn schemes(&self) -> &[String];
    /// Retrieve `request`, writing the bytes to its destination.
    async fn fetch(&self, request: &FetchRequest) -> Result<Fetched, FetchError>;

    /// Like [`Fetcher::fetch`], but report the cumulative number of retrieved
    /// bytes to `progress` as they arrive.
    ///
    /// The default implementation ignores `progress` and calls
    /// [`Fetcher::fetch`], so a fetcher that cannot observe progress need not
    /// implement this method. A caller that needs progress should use this
    /// method; a caller that does not can keep calling [`Fetcher::fetch`].
    ///
    /// `HttpFetcher` reports each received chunk; `CommandFetcher` polls the
    /// destination file size while its command runs.
    async fn fetch_with_progress(
        &self,
        request: &FetchRequest,
        progress: &ProgressFn<'_>,
    ) -> Result<Fetched, FetchError> {
        let _ = progress;
        self.fetch(request).await
    }
}

/// Dispatches an `Address` to the registered fetcher for its scheme.
#[derive(Default)]
pub struct FetchRegistry {
    fetchers: Vec<Arc<dyn Fetcher>>,
}

impl FetchRegistry {
    /// An empty registry with no fetchers.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registry with the built-in fetchers: `http`/`https`, an `ipfs` gateway
    /// fetcher when the `ipfs` feature is enabled, and a `file` fetcher when the
    /// `file` feature is enabled.
    ///
    /// The IPFS gateway is `DEFAULT_IPFS_GATEWAY` (an `IpfsGatewayFetcher`);
    /// build the registry yourself with `IpfsGatewayFetcher::new` to choose a
    /// different gateway. The `file` fetcher is built with `FileFetcher::new()`
    /// (any readable path); register a `FileFetcher::with_allowed_roots` yourself
    /// to restrict it.
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(HttpFetcher::new()));
        #[cfg(feature = "ipfs")]
        registry.register(Arc::new(IpfsGatewayFetcher::with_default_gateway()));
        #[cfg(feature = "file")]
        registry.register(Arc::new(FileFetcher::new()));
        registry
    }

    /// Register `fetcher`; its schemes become resolvable.
    pub fn register(&mut self, fetcher: Arc<dyn Fetcher>) -> &mut Self {
        self.fetchers.push(fetcher);
        self
    }

    /// The first registered fetcher that handles `scheme`, if any.
    pub fn fetcher_for(&self, scheme: &str) -> Option<Arc<dyn Fetcher>> {
        self.fetchers
            .iter()
            .find(|fetcher| fetcher.schemes().iter().any(|name| name == scheme))
            .cloned()
    }

    /// Fetch `address` into `destination`, verifying `expected_sha256` if given.
    pub async fn fetch(
        &self,
        address: &Address,
        destination: impl Into<PathBuf>,
        expected_sha256: Option<String>,
    ) -> Result<Fetched, FetchError> {
        self.fetch_with_progress(address, destination, expected_sha256, &|_: u64| {})
            .await
    }

    /// Like [`FetchRegistry::fetch`], but report cumulative retrieved bytes to
    /// `progress` as they arrive. See [`Fetcher::fetch_with_progress`].
    pub async fn fetch_with_progress(
        &self,
        address: &Address,
        destination: impl Into<PathBuf>,
        expected_sha256: Option<String>,
        progress: &ProgressFn<'_>,
    ) -> Result<Fetched, FetchError> {
        let fetcher = self
            .fetcher_for(&address.scheme)
            .ok_or_else(|| FetchError::Unsupported(address.scheme.clone()))?;
        fetcher
            .fetch_with_progress(
                &FetchRequest {
                    address: address.clone(),
                    destination: destination.into(),
                    expected_sha256,
                },
                progress,
            )
            .await
    }

    /// Fetch a merged result, trying its addresses in preference order until
    /// one succeeds (multi-source failover), and verifying its sha256.
    ///
    /// Addresses are ordered by scheme (`https` > `http` > `ipfs` > `magnet` >
    /// other), preserving the result's own order within a scheme; see
    /// [`preferred_addresses`]. The winning address is recorded on
    /// [`Fetched::address`]. Fails with [`FetchError::NoAddress`] when the
    /// result has no addresses, or [`FetchError::AllFailed`] (carrying every
    /// attempt's error) when all of them fail.
    pub async fn fetch_merged(
        &self,
        result: &MergedResult,
        destination: impl Into<PathBuf>,
    ) -> Result<Fetched, FetchError> {
        self.fetch_merged_with_progress(result, destination, &|_: u64| {})
            .await
    }

    /// Like [`FetchRegistry::fetch_merged`], but report cumulative retrieved
    /// bytes to `progress` as they arrive.
    ///
    /// The reported total is kept monotonic across failover: if one address
    /// fails part-way and the next succeeds, the bar never jumps backwards, and
    /// the final report equals the size of the address that won.
    pub async fn fetch_merged_with_progress(
        &self,
        result: &MergedResult,
        destination: impl Into<PathBuf>,
        progress: &ProgressFn<'_>,
    ) -> Result<Fetched, FetchError> {
        let destination = destination.into();
        let addresses = preferred_addresses(result);
        if addresses.is_empty() {
            return Err(FetchError::NoAddress);
        }

        // A failed attempt may already have reported bytes; clamp later reports
        // to the high-water mark so progress never regresses.
        let high_water = std::sync::atomic::AtomicU64::new(0);
        let mut errors = Vec::new();
        for address in addresses {
            let expected = expected_sha256(result, address);
            let monotonic = |bytes: u64| {
                let previous = high_water.fetch_max(bytes, std::sync::atomic::Ordering::Relaxed);
                if bytes > previous {
                    progress(bytes);
                }
            };
            match self
                .fetch_with_progress(address, destination.clone(), expected, &monotonic)
                .await
            {
                Ok(fetched) => return Ok(fetched),
                Err(error) => errors.push(error),
            }
        }
        let count = errors.len();
        Err(FetchError::AllFailed { count, errors })
    }
}

/// sha256 of a byte slice, as lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// sha256 of a file, as lowercase hex. Streams the file in chunks so a large
/// download is never read into memory at once.
pub async fn sha256_file(path: &Path) -> std::io::Result<String> {
    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Verify `actual` against an optional `expected` sha256 (case-insensitive).
pub fn verify_sha256(expected: Option<&str>, actual: &str) -> Result<(), FetchError> {
    match expected {
        Some(expected) if !expected.eq_ignore_ascii_case(actual) => Err(FetchError::HashMismatch {
            expected: expected.to_owned(),
            actual: actual.to_owned(),
        }),
        _ => Ok(()),
    }
}

/// Pick the preferred address of a result: `https` > `http` > `ipfs` > `magnet` > other.
///
/// The ordering is shared with [`crate::merge::suggest_filename`] so a suggested
/// filename and the address that is actually fetched agree.
pub fn preferred_address(result: &MergedResult) -> Option<&Address> {
    crate::merge::preferred_address(result)
}

/// Every address of a result, ordered by preference: `https` > `http` > `ipfs`
/// > `magnet` > other, preserving the result's own order within a scheme.
///
/// This is the order [`FetchRegistry::fetch_merged`] tries addresses in when it
/// fails over from a dead mirror.
pub fn preferred_addresses(result: &MergedResult) -> Vec<&Address> {
    let mut addresses: Vec<&Address> = result.addresses.iter().collect();
    addresses.sort_by_key(|address| crate::merge::address_rank(&address.scheme));
    addresses
}

/// The expected sha256 for a result/address, if either carries one.
pub fn expected_sha256(result: &MergedResult, address: &Address) -> Option<String> {
    result
        .hashes
        .get("sha256")
        .or_else(|| {
            address
                .hashes
                .as_ref()
                .and_then(|hashes| hashes.get("sha256"))
        })
        .cloned()
}

/// `<destination>.part` — written first, then renamed into place.
pub(crate) fn temp_path(destination: &Path) -> PathBuf {
    let mut name = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".to_owned());
    name.push_str(".part");
    destination.with_file_name(name)
}
