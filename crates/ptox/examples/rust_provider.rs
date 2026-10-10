//! A complete Rust [`SearchProvider`] example.
//!
//! It demonstrates the four things every provider author needs:
//!
//! 1. implementing [`SearchProvider`] with a streaming `search`;
//! 2. declaring [`Capabilities`] so the engine skips it for queries it cannot serve;
//! 3. mapping backend failures to the right [`ErrorCode`]; and
//! 4. keeping I/O behind a trait so the provider is testable **offline**.
//!
//! Run it — no network required, it uses an in-memory backend:
//!
//! ```sh
//! cargo run -p ptox --example rust_provider
//! ```
//!
//! See `docs/writing-a-provider.md` for the end-to-end guide and
//! `ptox-conformance` for the contract checks.

use async_trait::async_trait;
use ptox::search::{Capabilities, ProviderError, ResultStream, SearchProvider};
use ptox::types::{Address, ErrorCode, FileQuery, FileResult};
use std::sync::Arc;

/// One hit returned by the backing index. Deliberately network-agnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Hit {
    title: String,
    url: String,
    size: Option<u64>,
    mimetype: Option<String>,
}

/// The I/O half of the provider: whatever talks to the real index, search
/// service, or node.
///
/// Keeping it behind a trait is what makes the provider offline-testable — the
/// tests below drive the provider with a scripted fake instead of a network.
#[async_trait]
trait Backend: Send + Sync {
    /// Search the index for `text`.
    async fn search(&self, text: &str) -> Result<Vec<Hit>, BackendError>;
}

/// A backend failure. The provider maps each variant to a `ptox` [`ErrorCode`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum BackendError {
    /// The index could not be reached (socket, DNS, TLS, …).
    Unreachable(String),
    /// The index refused the request (rate limit, policy).
    Refused(String),
}

/// A provider over an arbitrary [`Backend`].
struct IndexProvider {
    name: String,
    backend: Arc<dyn Backend>,
}

impl IndexProvider {
    fn new(name: impl Into<String>, backend: Arc<dyn Backend>) -> Self {
        Self {
            name: name.into(),
            backend,
        }
    }
}

#[async_trait]
impl SearchProvider for IndexProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn capabilities(&self) -> Capabilities {
        // The engine will only ask this provider queries it can serve. `text`
        // is required; the filters are best-effort. Results are always HTTPS.
        Capabilities {
            query: vec!["text".into(), "mimetypes".into(), "extensions".into()],
            requires: vec!["text".into()],
            schemes: vec!["https".into()],
        }
    }

    fn search(&self, query: &FileQuery) -> ResultStream<'_> {
        // Clone what the stream needs so it owns its inputs and does not borrow
        // `query` (the trait ties the returned stream's lifetime to `&self`).
        let backend = self.backend.clone();
        let text = query.text.clone();

        Box::pin(async_stream::stream! {
            if text.trim().is_empty() {
                // Fail honestly instead of silently returning nothing.
                yield Err(ProviderError::new(
                    ErrorCode::unsupported(),
                    "demo-index needs a non-empty text query",
                ));
                return;
            }

            match backend.search(&text).await {
                Ok(hits) => {
                    for hit in hits {
                        let mut result = FileResult::new(
                            // Attribution is host-owned (`fanout` stamps it);
                            // a Rust provider leaves it empty.
                            String::new(),
                            vec![Address::new("https", hit.url)],
                        );
                        result.title = Some(hit.title);
                        result.size = hit.size;
                        result.mimetype = hit.mimetype;
                        yield Ok(result);
                    }
                }
                Err(BackendError::Unreachable(message)) => {
                    yield Err(ProviderError::new(ErrorCode::transport(), message));
                }
                Err(BackendError::Refused(message)) => {
                    yield Err(ProviderError::new(ErrorCode::refused(), message));
                }
            }
        })
    }
}

/// An in-memory backend used by the runnable example.
#[derive(Default)]
struct MemoryBackend {
    hits: Vec<Hit>,
}

#[async_trait]
impl Backend for MemoryBackend {
    async fn search(&self, text: &str) -> Result<Vec<Hit>, BackendError> {
        let needle = text.to_lowercase();
        Ok(self
            .hits
            .iter()
            .filter(|hit| hit.title.to_lowercase().contains(&needle))
            .cloned()
            .collect())
    }
}

/// A backend that always fails, used by the runnable example to show how a
/// provider maps backend failures onto ptox error codes.
struct BrokenBackend(BackendError);

#[async_trait]
impl Backend for BrokenBackend {
    async fn search(&self, _text: &str) -> Result<Vec<Hit>, BackendError> {
        Err(self.0.clone())
    }
}

#[tokio::main]
async fn main() {
    let backend = MemoryBackend {
        hits: vec![
            Hit {
                title: "annual report 2024.pdf".into(),
                url: "https://example.org/annual-2024.pdf".into(),
                size: Some(1_048_576),
                mimetype: Some("application/pdf".into()),
            },
            Hit {
                title: "annual report 2025.pdf".into(),
                url: "https://example.org/annual-2025.pdf".into(),
                size: Some(2_097_152),
                mimetype: Some("application/pdf".into()),
            },
            Hit {
                title: "quarterly notes.txt".into(),
                url: "https://example.org/notes.txt".into(),
                size: None,
                mimetype: None,
            },
        ],
    };

    let provider: Arc<dyn SearchProvider> =
        Arc::new(IndexProvider::new("demo-index", Arc::new(backend)));
    let offline: Arc<dyn SearchProvider> = Arc::new(IndexProvider::new(
        "offline-index",
        Arc::new(BrokenBackend(BackendError::Unreachable(
            "connection refused".into(),
        ))),
    ));
    let busy: Arc<dyn SearchProvider> = Arc::new(IndexProvider::new(
        "busy-index",
        Arc::new(BrokenBackend(BackendError::Refused(
            "rate limited; retry later".into(),
        ))),
    ));
    let engine = ptox::Engine::new(vec![provider, offline, busy]);

    let report = engine.search(FileQuery::new("annual report")).await;
    println!("{} result(s):", report.results.len());
    for result in &report.results {
        println!(
            "- {} [via {}]",
            result.title.as_deref().unwrap_or("<untitled>"),
            result.providers.join(", ")
        );
        for address in &result.addresses {
            println!("    {}", address.uri);
        }
    }
    for failure in &report.failures {
        eprintln!(
            "{} failed: {} ({})",
            failure.provider, failure.error.message, failure.error.code
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    /// A fake transport: it never touches the network and answers from a
    /// scripted list of hits, so the provider is tested deterministically.
    struct FakeBackend {
        hits: Vec<Hit>,
    }

    #[async_trait]
    impl Backend for FakeBackend {
        async fn search(&self, _text: &str) -> Result<Vec<Hit>, BackendError> {
            Ok(self.hits.clone())
        }
    }

    fn hit(title: &str) -> Hit {
        Hit {
            title: title.into(),
            url: format!("https://example.org/{title}"),
            size: None,
            mimetype: None,
        }
    }

    #[tokio::test]
    async fn maps_hits_and_leaves_attribution_to_the_host() {
        let backend = FakeBackend {
            hits: vec![hit("a.pdf")],
        };
        let provider = IndexProvider::new("demo-index", Arc::new(backend));
        let items = provider
            .search(&FileQuery::new("a"))
            .collect::<Vec<_>>()
            .await;

        assert_eq!(items.len(), 1);
        let result = items[0].as_ref().expect("one result");
        assert_eq!(result.title.as_deref(), Some("a.pdf"));
        assert_eq!(result.addresses[0].scheme, "https");
        assert!(result.provider.is_empty());
    }

    #[tokio::test]
    async fn maps_backend_failure_to_a_transport_error() {
        let backend = BrokenBackend(BackendError::Unreachable("connection refused".into()));
        let provider = IndexProvider::new("demo-index", Arc::new(backend));
        let items = provider
            .search(&FileQuery::new("a"))
            .collect::<Vec<_>>()
            .await;

        match &items[..] {
            [Err(error)] => {
                assert_eq!(error.code, ErrorCode::transport());
                assert_eq!(error.message, "connection refused");
            }
            other => panic!("expected one transport error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn maps_refusal_to_a_refused_error() {
        let backend = BrokenBackend(BackendError::Refused("slow down".into()));
        let provider = IndexProvider::new("demo-index", Arc::new(backend));
        let items = provider
            .search(&FileQuery::new("a"))
            .collect::<Vec<_>>()
            .await;

        match &items[..] {
            [Err(error)] => {
                assert_eq!(error.code, ErrorCode::refused());
                assert_eq!(error.message, "slow down");
            }
            other => panic!("expected one refused error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn empty_text_is_reported_as_unsupported() {
        let backend = FakeBackend {
            hits: vec![hit("a.pdf")],
        };
        let provider = IndexProvider::new("demo-index", Arc::new(backend));
        let items = provider
            .search(&FileQuery::new("   "))
            .collect::<Vec<_>>()
            .await;

        match &items[..] {
            [Err(error)] => assert_eq!(error.code, ErrorCode::unsupported()),
            other => panic!("expected one unsupported error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn capabilities_require_text() {
        let backend = FakeBackend { hits: Vec::new() };
        let provider = IndexProvider::new("demo-index", Arc::new(backend));
        let capabilities = provider.capabilities().await;

        assert!(capabilities.accepts(&FileQuery::new("hello")));
        assert!(!capabilities.accepts(&FileQuery::new("")));
    }

    #[tokio::test]
    async fn engine_stamps_attribution() {
        let backend = FakeBackend {
            hits: vec![hit("a.pdf")],
        };
        let provider: Arc<dyn SearchProvider> =
            Arc::new(IndexProvider::new("demo-index", Arc::new(backend)));
        let engine = ptox::Engine::new(vec![provider]);

        let report = engine.search(FileQuery::new("a")).await;
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].providers, vec!["demo-index".to_owned()]);
    }
}
