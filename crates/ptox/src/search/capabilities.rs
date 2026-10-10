//! What a provider can do, so hosts can route queries and present capabilities.

use crate::types::FileQuery;
use serde::{Deserialize, Serialize};

/// A provider's declared abilities.
///
/// The default is **universal**: an empty `query` means "no constraint" and an
/// empty `requires` means "no requirement". A provider that doesn't declare
/// anything is treated as able to handle any query.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// Query facets the provider can honor. Empty means "any". Known facets:
    /// `text`, `hashes`, `mimetypes`, `extensions`, `filename`, `size`,
    /// `published`, `lang`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub query: Vec<String>,
    /// Facets that MUST be present for the provider to accept a query
    /// (e.g. an infohash-only provider `requires: ["hashes"]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    /// Address schemes the provider can emit. Empty means "any".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schemes: Vec<String>,
}

impl Capabilities {
    /// A provider that accepts anything (the trait default).
    pub fn universal() -> Self {
        Self::default()
    }

    fn used(query: &FileQuery) -> Vec<&'static str> {
        let mut used = Vec::new();
        if !query.text.is_empty() {
            used.push("text");
        }
        if query.hashes.is_some() {
            used.push("hashes");
        }
        if query.mimetypes.is_some() {
            used.push("mimetypes");
        }
        if query.extensions.is_some() {
            used.push("extensions");
        }
        if query.filename.is_some() {
            used.push("filename");
        }
        if query.size_min.is_some() || query.size_max.is_some() {
            used.push("size");
        }
        if query.published_after.is_some() || query.published_before.is_some() {
            used.push("published");
        }
        if query.lang.is_some() {
            used.push("lang");
        }
        used
    }

    /// Whether an emitted address scheme is allowed by this provider.
    ///
    /// An empty `schemes` list means "any" (the universal default), so every
    /// scheme is allowed. Clients and conformance tools use this to check that a
    /// provider only emits addresses it declared.
    pub fn allows_scheme(&self, scheme: &str) -> bool {
        self.schemes.is_empty() || self.schemes.iter().any(|allowed| allowed == scheme)
    }

    /// Whether this provider should be asked a given query.
    pub fn accepts(&self, query: &FileQuery) -> bool {
        let used = Self::used(query);
        if self
            .requires
            .iter()
            .any(|required| !used.iter().any(|facet| facet == required))
        {
            return false;
        }
        used.iter()
            .all(|facet| self.query.is_empty() || self.query.iter().any(|allowed| allowed == facet))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn universal_accepts_everything() {
        let capabilities = Capabilities::universal();
        assert!(capabilities.accepts(&FileQuery::new("anything")));
        let mut with_hash = FileQuery::new("");
        with_hash.hashes = Some([("infohash".to_owned(), "abc".to_owned())].into());
        assert!(capabilities.accepts(&with_hash));
    }

    #[test]
    fn requires_facets_are_enforced() {
        let capabilities = Capabilities {
            query: vec!["hashes".to_owned()],
            requires: vec!["hashes".to_owned()],
            schemes: vec!["magnet".to_owned()],
        };
        assert!(!capabilities.accepts(&FileQuery::new("annual report")));

        let mut with_hash = FileQuery::new("");
        with_hash.hashes = Some([("infohash".to_owned(), "abc".to_owned())].into());
        assert!(capabilities.accepts(&with_hash));
    }

    #[test]
    fn unsupported_facets_are_rejected() {
        let capabilities = Capabilities {
            query: vec!["text".to_owned()],
            ..Capabilities::default()
        };
        assert!(capabilities.accepts(&FileQuery::new("text only")));

        let mut with_type = FileQuery::new("x");
        with_type.mimetypes = Some(vec!["image/*".to_owned()]);
        assert!(!capabilities.accepts(&with_type));
    }

    #[test]
    fn scheme_less_is_universal() {
        let capabilities = Capabilities::universal();
        assert!(capabilities.allows_scheme("https"));
        assert!(capabilities.allows_scheme("magnet"));
        assert!(capabilities.allows_scheme("i2p"));
    }

    #[test]
    fn declared_schemes_are_enforced() {
        let capabilities = Capabilities {
            schemes: vec!["https".to_owned(), "ipfs".to_owned()],
            ..Capabilities::default()
        };
        assert!(capabilities.allows_scheme("https"));
        assert!(capabilities.allows_scheme("ipfs"));
        assert!(!capabilities.allows_scheme("magnet"));
    }
}
