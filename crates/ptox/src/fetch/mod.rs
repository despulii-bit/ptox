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
mod http;

pub use command::CommandFetcher;
pub use http::HttpFetcher;

use crate::merge::MergedResult;
use crate::types::Address;
use async_trait::async_trait;
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
    #[error("result has no address")]
    NoAddress,
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
    fn schemes(&self) -> &[String];
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
            .find(|fetcher| fetcher.schemes().iter().any(|name| name == scheme))
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

    /// Fetch the preferred address of a merged result, verifying its sha256.
    pub async fn fetch_merged(
        &self,
        result: &MergedResult,
        destination: impl Into<PathBuf>,
    ) -> Result<Fetched, FetchError> {
        let address = preferred_address(result).ok_or(FetchError::NoAddress)?;
        self.fetch(address, destination, expected_sha256(result, address))
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

/// Pick the preferred address of a result: `https` > `http` > `ipfs` > `magnet` > other.
pub fn preferred_address(result: &MergedResult) -> Option<&Address> {
    result
        .addresses
        .iter()
        .min_by_key(|address| match address.scheme.as_str() {
            "https" => 0,
            "http" => 1,
            "ipfs" => 2,
            "magnet" => 3,
            _ => 4,
        })
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
