//! The data contracts shared by every ptox component: queries, results,
//! addresses, failures, and the structural validation used across the wire
//! boundary.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Provider-specific passthrough / forward-compatible extras.
///
/// Keys SHOULD be namespaced (`provider.key`) to avoid collisions between
/// independent providers.
pub type Extras = serde_json::Map<String, serde_json::Value>;

/// Map of hash algorithm name to value, e.g. `{"sha256": "...", "infohash": "..."}`.
pub type HashAlgos = HashMap<String, String>;

/// The one network-agnostic query shape.
///
/// Every selector is optional except `text`; providers apply what they support
/// best-effort and must still return text matches when a filter is unsupported.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileQuery {
    /// Free-text keywords. May be empty when another selector is present.
    pub text: String,
    /// Content hashes to look up directly, keyed by algorithm name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hashes: Option<HashAlgos>,
    /// Accepted media types: full (`application/pdf`), top-level (`image`), or
    /// wildcarded (`image/*`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mimetypes: Option<Vec<String>>,
    /// Accepted file extensions, lowercase, no leading dot (`pdf`, `jpg`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Vec<String>>,
    /// Filename or glob pattern (e.g. `*annual*report*.pdf`). Best-effort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// Minimum size in bytes, inclusive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_min: Option<u64>,
    /// Maximum size in bytes, inclusive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_max: Option<u64>,
    /// Only results published at/after this instant (RFC 3339 UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_after: Option<String>,
    /// Only results published at/before this instant (RFC 3339 UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_before: Option<String>,
    /// Preferred language (BCP 47, e.g. `en`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// Max results per provider (best-effort).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Provider-specific extras; the core never interprets them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extras: Option<Extras>,
}

impl FileQuery {
    /// A query for `text`, with every other selector unset.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            hashes: None,
            mimetypes: None,
            extensions: None,
            filename: None,
            size_min: None,
            size_max: None,
            published_after: None,
            published_before: None,
            lang: None,
            limit: None,
            extras: None,
        }
    }
}

/// A way to obtain content. This is the seam between search and retrieval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Address {
    /// Dispatch key. Open set: `http`, `magnet`, `ipfs`, `i2p`, `freenet`, ...
    pub scheme: String,
    /// The URI a fetcher for `scheme` knows how to resolve.
    pub uri: String,
    /// Content hashes, keyed by algorithm name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hashes: Option<HashAlgos>,
    /// Size in bytes, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Provider-specific extras; ignored by the core.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extras: Option<Extras>,
}

impl Address {
    /// An address with `scheme` and `uri` set and no optional metadata.
    pub fn new(scheme: impl Into<String>, uri: impl Into<String>) -> Self {
        Self {
            scheme: scheme.into(),
            uri: uri.into(),
            hashes: None,
            size: None,
            extras: None,
        }
    }
}

/// A normalized, attributed search hit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileResult {
    /// Attribution: which provider produced it. Never empty once filled.
    /// Providers MAY omit this on the wire; the host sets it (contract §9).
    #[serde(default)]
    pub provider: String,
    /// Human-readable title, if the provider has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Ways to obtain the content. Never empty.
    pub addresses: Vec<Address>,
    /// Size in bytes, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Content hashes, keyed by algorithm name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hashes: Option<HashAlgos>,
    /// Media type of the content (e.g. `application/pdf`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mimetype: Option<String>,
    /// Language of the content (BCP 47).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// When the content was published (RFC 3339 UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
    /// When the provider last saw it available (RFC 3339 UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<String>,
    /// Provider-local relevance. NOT comparable across providers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    /// Untouched original payload. Never interpreted by the core.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<serde_json::Value>,
}

impl FileResult {
    /// A result from `provider` obtainable via `addresses`.
    pub fn new(provider: impl Into<String>, addresses: Vec<Address>) -> Self {
        Self {
            provider: provider.into(),
            title: None,
            addresses,
            size: None,
            hashes: None,
            mimetype: None,
            lang: None,
            published: None,
            last_seen: None,
            score: None,
            raw: None,
        }
    }
}

/// A well-known or custom provider error code. Unknown values are tolerated.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ErrorCode(pub String);

impl ErrorCode {
    /// Wrap an arbitrary code string.
    pub fn new(code: impl Into<String>) -> Self {
        Self(code.into())
    }
    /// The provider exceeded its time budget.
    pub fn timeout() -> Self {
        Self("timeout".into())
    }
    /// The provider cannot answer this query.
    pub fn unsupported() -> Self {
        Self("unsupported".into())
    }
    /// The provider refused to answer (policy, rate limit).
    pub fn refused() -> Self {
        Self("refused".into())
    }
    /// A transport/network failure.
    pub fn transport() -> Self {
        Self("transport".into())
    }
    /// The provider produced output that violated the wire contract.
    pub fn malformed() -> Self {
        Self("malformed".into())
    }
    /// The query was cancelled before it finished.
    pub fn cancelled() -> Self {
        Self("cancelled".into())
    }
    /// An error internal to the provider.
    pub fn internal() -> Self {
        Self("internal".into())
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ErrorCode {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for ErrorCode {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// A machine-readable error code plus a human-readable message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorInfo {
    /// The error category (see [`ErrorCode`]).
    pub code: ErrorCode,
    /// A short, non-empty explanation. Never carries secrets.
    pub message: String,
}

/// A per-provider failure. One provider failing never affects the others.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    /// The provider that failed.
    pub provider: String,
    /// What went wrong.
    pub error: ErrorInfo,
}

/// What the engine streams: either a hit or a provider failure.
#[allow(clippy::large_enum_variant)] // a ~280-byte move beats a per-item allocation
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// A search hit.
    Result(FileResult),
    /// A provider-level failure.
    Failure(Failure),
}

impl Outcome {
    /// The provider this outcome is attributed to.
    pub fn provider(&self) -> &str {
        match self {
            Outcome::Result(result) => &result.provider,
            Outcome::Failure(failure) => &failure.provider,
        }
    }

    /// The hit, if this is a [`Outcome::Result`].
    pub fn as_result(&self) -> Option<&FileResult> {
        match self {
            Outcome::Result(result) => Some(result),
            Outcome::Failure(_) => None,
        }
    }

    /// The failure, if this is a [`Outcome::Failure`].
    pub fn as_failure(&self) -> Option<&Failure> {
        match self {
            Outcome::Result(_) => None,
            Outcome::Failure(failure) => Some(failure),
        }
    }
}

/// Structural validation for contract types.
pub trait Validate {
    /// Check the invariants the contract requires for this value.
    fn validate(&self) -> Result<(), ValidationError>;
}

/// The ways a contract value can be structurally invalid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    /// A field that must be non-empty is empty.
    #[error("field `{0}` must not be empty")]
    Empty(&'static str),
    /// A result carried no addresses.
    #[error("`addresses` must not be empty")]
    NoAddresses,
    /// A query's `size_min` was greater than its `size_max`.
    #[error("size_min ({min}) must not exceed size_max ({max})")]
    SizeRange {
        /// The lower bound.
        min: u64,
        /// The upper bound.
        max: u64,
    },
}

impl Validate for FileQuery {
    fn validate(&self) -> Result<(), ValidationError> {
        if let (Some(min), Some(max)) = (self.size_min, self.size_max) {
            if min > max {
                return Err(ValidationError::SizeRange { min, max });
            }
        }
        if let Some(mimetypes) = &self.mimetypes {
            if mimetypes.iter().any(|mimetype| mimetype.trim().is_empty()) {
                return Err(ValidationError::Empty("mimetypes entry"));
            }
        }
        if let Some(extensions) = &self.extensions {
            if extensions
                .iter()
                .any(|extension| extension.trim().is_empty())
            {
                return Err(ValidationError::Empty("extensions entry"));
            }
        }
        if let Some(filename) = &self.filename {
            if filename.trim().is_empty() {
                return Err(ValidationError::Empty("filename"));
            }
        }
        if let Some(lang) = &self.lang {
            if lang.trim().is_empty() {
                return Err(ValidationError::Empty("lang"));
            }
        }
        Ok(())
    }
}

impl Validate for Address {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.scheme.is_empty() {
            return Err(ValidationError::Empty("scheme"));
        }
        if self.uri.is_empty() {
            return Err(ValidationError::Empty("uri"));
        }
        Ok(())
    }
}

impl Validate for FileResult {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.provider.is_empty() {
            return Err(ValidationError::Empty("provider"));
        }
        if self.addresses.is_empty() {
            return Err(ValidationError::NoAddresses);
        }
        for address in &self.addresses {
            address.validate()?;
        }
        Ok(())
    }
}

impl Validate for Failure {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.provider.is_empty() {
            return Err(ValidationError::Empty("provider"));
        }
        Ok(())
    }
}
