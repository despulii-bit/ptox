use async_trait::async_trait;
use ptox::{
    Address, Capabilities, Engine, FileQuery, FileResult, ProviderError, ResultStream,
    SearchProvider,
};
use std::collections::HashMap;
use std::sync::Arc;

struct Mock {
    result: FileResult,
}

impl SearchProvider for Mock {
    fn name(&self) -> &str {
        &self.result.provider
    }

    fn search(&self, _query: &FileQuery) -> ResultStream<'_> {
        let result = self.result.clone();
        Box::pin(async_stream::stream! { yield Ok(result); })
    }
}

struct Failing;

impl SearchProvider for Failing {
    fn name(&self) -> &str {
        "failing"
    }

    fn search(&self, _query: &FileQuery) -> ResultStream<'_> {
        Box::pin(async_stream::stream! { yield Err(ProviderError::new("refused", "nope")); })
    }
}

/// Accepts only hash queries (like an infohash-only network).
struct HashOnly;

#[async_trait]
impl SearchProvider for HashOnly {
    fn name(&self) -> &str {
        "hash-only"
    }

    async fn capabilities(&self) -> Capabilities {
        Capabilities {
            query: vec!["hashes".to_owned()],
            requires: vec!["hashes".to_owned()],
            schemes: vec!["magnet".to_owned()],
        }
    }

    fn search(&self, _query: &FileQuery) -> ResultStream<'_> {
        Box::pin(async_stream::stream! {
            yield Ok(FileResult::new(
                "hash-only",
                vec![Address::new("magnet", "magnet:?xt=urn:btih:abc")],
            ));
        })
    }
}

fn result(provider: &str, sha: &str) -> FileResult {
    let mut result = FileResult::new(
        provider,
        vec![Address::new("https", format!("https://{provider}/f"))],
    );
    result.title = Some("f.bin".to_owned());
    result.size = Some(10);
    result.hashes = Some(HashMap::from([("sha256".to_owned(), sha.to_owned())]));
    result
}

#[tokio::test]
async fn engine_searches_and_merges_across_providers() {
    let providers: Vec<Arc<dyn SearchProvider>> = vec![
        Arc::new(Mock {
            result: result("a", "deadbeef"),
        }),
        Arc::new(Mock {
            result: result("b", "deadbeef"),
        }),
    ];
    let engine = Engine::new(providers);
    let report = engine.search(FileQuery::new("x")).await;

    assert_eq!(report.results.len(), 1);
    assert_eq!(report.results[0].providers, vec!["a", "b"]);
    assert!(report.failures.is_empty());
}

#[tokio::test]
async fn engine_reports_provider_failures() {
    let providers: Vec<Arc<dyn SearchProvider>> = vec![
        Arc::new(Mock {
            result: result("a", "1111"),
        }),
        Arc::new(Failing),
    ];
    let engine = Engine::new(providers);
    let report = engine.search(FileQuery::new("x")).await;

    assert_eq!(report.results.len(), 1);
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].provider, "failing");
}

#[tokio::test]
async fn engine_skips_incapable_providers() {
    let providers: Vec<Arc<dyn SearchProvider>> = vec![
        Arc::new(Mock {
            result: result("a", "1111"),
        }),
        Arc::new(HashOnly),
    ];
    let engine = Engine::new(providers);

    let keyword = engine.search(FileQuery::new("annual report")).await;
    assert_eq!(keyword.skipped, vec!["hash-only"]);
    assert_eq!(keyword.results.len(), 1);

    let mut hash_query = FileQuery::new("");
    hash_query.hashes = Some(HashMap::from([("infohash".to_owned(), "abc".to_owned())]));
    let hash = engine.search(hash_query).await;
    assert!(hash.skipped.is_empty());
}
