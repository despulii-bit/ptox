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
///
/// Build one fluently with [`FileQuery::new`] followed by the `with_*` methods
/// (`with_mimetype`, `with_extension`, `with_size_range`, …). The fields are
/// public, so direct assignment works too.
///
/// ```
/// use ptox::FileQuery;
///
/// let query = FileQuery::new("annual report")
///     .with_mimetype("application/pdf")
///     .with_extension(".PDF")
///     .with_size_range(0, 10 * 1024 * 1024)
///     .with_lang("en")
///     .with_offset(50)
///     .with_limit(25);
///
/// assert_eq!(query.mimetypes, Some(vec!["application/pdf".to_owned()]));
/// assert_eq!(query.extensions, Some(vec!["pdf".to_owned()]));
/// assert_eq!(query.size_max, Some(10 * 1024 * 1024));
/// assert_eq!(query.offset, Some(50));
/// assert_eq!(query.limit, Some(25));
/// ```
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
    /// Results to skip per provider before `limit` (best-effort pagination).
    ///
    /// The counterpart to [`limit`](Self::limit): a provider skips its first
    /// `offset` matches, then returns at most `limit`. Like every selector it is
    /// best-effort, and it is pagination rather than a filter, so it is not a
    /// [`Capabilities`](crate::search::Capabilities) facet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
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
            offset: None,
            extras: None,
        }
    }

    /// Set [`hashes`](Self::hashes), replacing any existing selectors. An empty
    /// map clears the field.
    pub fn with_hashes(mut self, hashes: HashAlgos) -> Self {
        self.hashes = (!hashes.is_empty()).then_some(hashes);
        self
    }

    /// Add one hash selector (e.g. `("infohash", "…")`), keeping any already
    /// set.
    pub fn with_hash(mut self, algorithm: impl Into<String>, value: impl Into<String>) -> Self {
        self.hashes
            .get_or_insert_with(HashMap::new)
            .insert(algorithm.into(), value.into());
        self
    }

    /// Set [`mimetypes`](Self::mimetypes), replacing any existing values. An
    /// empty iterator clears the field.
    pub fn with_mimetypes<I, S>(mut self, mimetypes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mimetypes: Vec<String> = mimetypes.into_iter().map(Into::into).collect();
        self.mimetypes = (!mimetypes.is_empty()).then_some(mimetypes);
        self
    }

    /// Add one accepted media type (full, top-level, or wildcarded), keeping any
    /// already set.
    pub fn with_mimetype(mut self, mimetype: impl Into<String>) -> Self {
        self.mimetypes
            .get_or_insert_with(Vec::new)
            .push(mimetype.into());
        self
    }

    /// Set [`extensions`](Self::extensions), replacing any existing values. Each
    /// entry is normalized (trimmed, lowercased, and with leading dots removed).
    /// An empty iterator clears the field.
    pub fn with_extensions<I, S>(mut self, extensions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let extensions: Vec<String> = extensions
            .into_iter()
            .map(|extension| normalize_extension(extension.into()))
            .collect();
        self.extensions = (!extensions.is_empty()).then_some(extensions);
        self
    }

    /// Add one accepted file extension, keeping any already set. The value is
    /// normalized, so `.PDF` and `pdf` both become `pdf`.
    pub fn with_extension(mut self, extension: impl Into<String>) -> Self {
        self.extensions
            .get_or_insert_with(Vec::new)
            .push(normalize_extension(extension.into()));
        self
    }

    /// Set [`filename`](Self::filename): a filename or best-effort glob.
    pub fn with_filename(mut self, filename: impl Into<String>) -> Self {
        self.filename = Some(filename.into());
        self
    }

    /// Set both [`size_min`](Self::size_min) and [`size_max`](Self::size_max),
    /// inclusive.
    pub fn with_size_range(mut self, min: u64, max: u64) -> Self {
        self.size_min = Some(min);
        self.size_max = Some(max);
        self
    }

    /// Set the inclusive lower size bound ([`size_min`](Self::size_min)).
    pub fn with_size_min(mut self, min: u64) -> Self {
        self.size_min = Some(min);
        self
    }

    /// Set the inclusive upper size bound ([`size_max`](Self::size_max)).
    pub fn with_size_max(mut self, max: u64) -> Self {
        self.size_max = Some(max);
        self
    }

    /// Set the earliest accepted publication instant
    /// ([`published_after`](Self::published_after), RFC 3339 UTC).
    pub fn with_published_after(mut self, instant: impl Into<String>) -> Self {
        self.published_after = Some(instant.into());
        self
    }

    /// Set the latest accepted publication instant
    /// ([`published_before`](Self::published_before), RFC 3339 UTC).
    pub fn with_published_before(mut self, instant: impl Into<String>) -> Self {
        self.published_before = Some(instant.into());
        self
    }

    /// Set the preferred language ([`lang`](Self::lang), BCP 47, e.g. `en`).
    pub fn with_lang(mut self, lang: impl Into<String>) -> Self {
        self.lang = Some(lang.into());
        self
    }

    /// Set the best-effort per-provider result cap ([`limit`](Self::limit)).
    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Set the best-effort per-provider result skip ([`offset`](Self::offset)),
    /// applied before [`limit`](Self::limit), so `offset` + `limit` select one
    /// page: `with_offset(page * size).with_limit(size)`.
    pub fn with_offset(mut self, offset: u32) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Set [`extras`](Self::extras), replacing any existing map. An empty map
    /// clears the field.
    pub fn with_extras(mut self, extras: Extras) -> Self {
        self.extras = (!extras.is_empty()).then_some(extras);
        self
    }

    /// Add one provider-specific extra, keeping any already set. Keys SHOULD be
    /// namespaced (`provider.key`).
    pub fn with_extra(
        mut self,
        key: impl Into<String>,
        value: impl Into<serde_json::Value>,
    ) -> Self {
        self.extras
            .get_or_insert_with(Extras::new)
            .insert(key.into(), value.into());
        self
    }
}

/// Normalize a file extension to the contract's lowercased, dotless form.
fn normalize_extension(extension: String) -> String {
    extension
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase()
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

/// How a source makes a result available.
///
/// Factual metadata a provider MAY set — **not** a policy: the core never blocks a
/// fetch, and a client may filter or sort on it. `None` on a result means the
/// provider did not say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    /// A direct download, no account or purchase.
    Download,
    /// Available by borrowing or with an account (e.g. a lending library).
    Borrow,
    /// A preview or snippet only.
    Preview,
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
    /// How the source offers it (see [`Access`]), when the provider says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<Access>,
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
            access: None,
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
    /// The provider (or its backing service) is not ready yet — for example a
    /// network daemon that is still bootstrapping. Temporary: a client should
    /// retry rather than treat it as a hard failure.
    pub fn unavailable() -> Self {
        Self("unavailable".into())
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

#[cfg(test)]
mod builder_tests {
    use super::*;

    #[test]
    fn with_helpers_set_every_selector() {
        let query = FileQuery::new("annual report")
            .with_hash("sha256", "abc")
            .with_hash("infohash", "def")
            .with_mimetype("image/*")
            .with_mimetypes(["audio/*", "video/*"])
            .with_extension("JPG")
            .with_extensions(["epub", ".TXT"])
            .with_filename("*report*")
            .with_size_range(10, 99)
            .with_published_after("2020-01-01T00:00:00Z")
            .with_published_before("2021-01-01T00:00:00Z")
            .with_lang("en")
            .with_offset(10)
            .with_limit(5)
            .with_extra("lab.foo", "bar")
            .with_extra("lab.count", 3);

        assert_eq!(query.text, "annual report");
        let hashes = query.hashes.as_ref().unwrap();
        assert_eq!(hashes["sha256"], "abc");
        assert_eq!(hashes["infohash"], "def");
        assert_eq!(
            query.mimetypes.as_ref().unwrap(),
            &vec!["audio/*".to_owned(), "video/*".to_owned()]
        );
        assert_eq!(
            query.extensions.as_ref().unwrap(),
            &vec!["epub".to_owned(), "txt".to_owned()]
        );
        assert_eq!(query.filename.as_deref(), Some("*report*"));
        assert_eq!(query.size_min, Some(10));
        assert_eq!(query.size_max, Some(99));
        assert_eq!(
            query.published_after.as_deref(),
            Some("2020-01-01T00:00:00Z")
        );
        assert_eq!(
            query.published_before.as_deref(),
            Some("2021-01-01T00:00:00Z")
        );
        assert_eq!(query.lang.as_deref(), Some("en"));
        assert_eq!(query.limit, Some(5));
        assert_eq!(query.offset, Some(10));
        let extras = query.extras.as_ref().unwrap();
        assert_eq!(extras["lab.foo"], serde_json::json!("bar"));
        assert_eq!(extras["lab.count"], serde_json::json!(3));
        query.validate().unwrap();
    }

    #[test]
    fn empty_collections_clear_fields() {
        let query = FileQuery::new("x")
            .with_hashes(HashAlgos::new())
            .with_mimetypes(Vec::<String>::new())
            .with_extensions(Vec::<String>::new())
            .with_extras(Extras::new());

        assert!(query.hashes.is_none());
        assert!(query.mimetypes.is_none());
        assert!(query.extensions.is_none());
        assert!(query.extras.is_none());
    }

    #[test]
    fn extension_helpers_normalize() {
        let query = FileQuery::new("x").with_extension("  .TaR.Gz  ");
        assert_eq!(
            query.extensions.as_deref(),
            Some(["tar.gz".to_owned()].as_slice())
        );
    }

    #[test]
    fn size_bounds_are_independent() {
        let query = FileQuery::new("x").with_size_min(1).with_size_max(2);
        assert_eq!(query.size_min, Some(1));
        assert_eq!(query.size_max, Some(2));
    }
}
