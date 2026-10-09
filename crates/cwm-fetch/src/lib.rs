//! # cwm-fetch
//!
//! The retrieval half of cwm: turn a [`cwm_types::Address`] into bytes on disk.
//!
//! The search fabric emits `Address`es; this fabric consumes them. Fetchers are
//! registered per scheme, so retrieval is pluggable exactly like search.
//!
//! ```no_run
//! # async fn run() -> Result<(), cwm_fetch::FetchError> {
//! use cwm_types::Address;
//! let registry = cwm_fetch::FetchRegistry::with_defaults();
//! let address = Address::new("https", "https://example.org/file.bin");
//! let fetched = registry.fetch(&address, "file.bin", None).await?;
//! println!("{} bytes -> {}", fetched.bytes, fetched.path.display());
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]

mod command;
mod http;

pub use command::CommandFetcher;
pub use http::HttpFetcher;

use async_trait::async_trait;
use cwm_types::Address;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FetchError {
    #[error("unsupported address scheme `{0}`")]
    Unsupported(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("network error: {0}")]
    Network(String),
    #[error("command failed: {0}")]
    Command(String),
    #[error("hash mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },
}

#[derive(Debug, Clone)]
pub struct FetchRequest {
    pub address: Address,
    pub destination: PathBuf,
    /// Expected sha256 (lowercase hex); verified after retrieval.
    pub expected_sha256: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Fetched {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: Option<String>,
}

/// One way of turning an address into bytes (one protocol).
#[async_trait]
pub trait Fetcher: Send + Sync {
    fn name(&self) -> &str;
    fn schemes(&self) -> &'static [&'static str];
    async fn fetch(&self, request: &FetchRequest) -> Result<Fetched, FetchError>;
}

/// Dispatches an `Address` to the registered fetcher for its scheme.
#[derive(Default)]
pub struct FetchRegistry {
    fetchers: Vec<Arc<dyn Fetcher>>,
}

impl FetchRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registry with the built-in `http`/`https` fetcher.
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(HttpFetcher::new()));
        registry
    }

    pub fn register(&mut self, fetcher: Arc<dyn Fetcher>) -> &mut Self {
        self.fetchers.push(fetcher);
        self
    }

    pub fn fetcher_for(&self, scheme: &str) -> Option<Arc<dyn Fetcher>> {
        self.fetchers
            .iter()
            .find(|fetcher| fetcher.schemes().contains(&scheme))
            .cloned()
    }

    pub async fn fetch(
        &self,
        address: &Address,
        destination: impl Into<PathBuf>,
        expected_sha256: Option<String>,
    ) -> Result<Fetched, FetchError> {
        let fetcher = self
            .fetcher_for(&address.scheme)
            .ok_or_else(|| FetchError::Unsupported(address.scheme.clone()))?;
        fetcher
            .fetch(&FetchRequest {
                address: address.clone(),
                destination: destination.into(),
                expected_sha256,
            })
            .await
    }
}

/// sha256 of a byte slice, as lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// sha256 of a file, as lowercase hex.
pub async fn sha256_file(path: &Path) -> std::io::Result<String> {
    let bytes = tokio::fs::read(path).await?;
    Ok(sha256_hex(&bytes))
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

/// `<destination>.part` — written first, then renamed into place.
pub(crate) fn temp_path(destination: &Path) -> PathBuf {
    let mut name = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".to_owned());
    name.push_str(".part");
    destination.with_file_name(name)
}
