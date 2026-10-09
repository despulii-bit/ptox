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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_min: Option<u64>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extras: Option<Extras>,
}

impl FileQuery {
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
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hashes: Option<HashAlgos>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extras: Option<Extras>,
}

impl Address {
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Ways to obtain the content. Never empty.
    pub addresses: Vec<Address>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hashes: Option<HashAlgos>,
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
    pub fn new(code: impl Into<String>) -> Self {
        Self(code.into())
    }
    pub fn timeout() -> Self {
        Self("timeout".into())
    }
    pub fn unsupported() -> Self {
        Self("unsupported".into())
    }
    pub fn refused() -> Self {
        Self("refused".into())
    }
    pub fn transport() -> Self {
        Self("transport".into())
    }
    pub fn malformed() -> Self {
        Self("malformed".into())
    }
    pub fn cancelled() -> Self {
        Self("cancelled".into())
    }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorInfo {
    pub code: ErrorCode,
    pub message: String,
}

/// A per-provider failure. One provider failing never affects the others.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub provider: String,
    pub error: ErrorInfo,
}

/// What the engine streams: either a hit or a provider failure.
#[allow(clippy::large_enum_variant)] // a ~280-byte move beats a per-item allocation
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Result(FileResult),
    Failure(Failure),
}

impl Outcome {
    pub fn provider(&self) -> &str {
        match self {
            Outcome::Result(result) => &result.provider,
            Outcome::Failure(failure) => &failure.provider,
        }
    }

    pub fn as_result(&self) -> Option<&FileResult> {
        match self {
            Outcome::Result(result) => Some(result),
            Outcome::Failure(_) => None,
        }
    }

    pub fn as_failure(&self) -> Option<&Failure> {
        match self {
            Outcome::Result(_) => None,
            Outcome::Failure(failure) => Some(failure),
        }
    }
}

/// Structural validation for contract types.
pub trait Validate {
    fn validate(&self) -> Result<(), ValidationError>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    #[error("field `{0}` must not be empty")]
    Empty(&'static str),
    #[error("`addresses` must not be empty")]
    NoAddresses,
    #[error("size_min ({min}) must not exceed size_max ({max})")]
    SizeRange { min: u64, max: u64 },
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
