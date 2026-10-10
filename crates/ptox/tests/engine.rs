use async_trait::async_trait;
use futures::StreamExt;
use ptox::{
    Address, Capabilities, Engine, FileQuery, FileResult, ProviderError, ResultStream,
    SearchProvider,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
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

/// Pages its fixed list using the query's `offset`/`limit`, as a real backend would.
struct Paged {
    results: Vec<FileResult>,
}

impl SearchProvider for Paged {
    fn name(&self) -> &str {
        "paged"
    }

    fn search(&self, query: &FileQuery) -> ResultStream<'_> {
        let offset = query.offset.unwrap_or(0) as usize;
        let limit = query
            .limit
            .map(|limit| limit as usize)
            .unwrap_or(usize::MAX);
        let page: Vec<FileResult> = self
            .results
            .iter()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect();
        Box::pin(async_stream::stream! {
            for result in page {
                yield Ok(result);
            }
        })
    }
}

/// Counts how often its capabilities are computed and invalidated.
struct Counting {
    calls: Arc<AtomicUsize>,
    invalidations: Arc<AtomicUsize>,
}

impl Counting {
    fn new() -> (Self, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let invalidations = Arc::new(AtomicUsize::new(0));
        (
            Self {
                calls: calls.clone(),
                invalidations: invalidations.clone(),
            },
            calls,
            invalidations,
        )
    }
}

#[async_trait]
impl SearchProvider for Counting {
    fn name(&self) -> &str {
        "counting"
    }

    async fn capabilities(&self) -> Capabilities {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Capabilities {
            query: vec!["text".to_owned()],
            ..Capabilities::default()
        }
    }

    fn invalidate_capabilities(&self) {
        self.invalidations.fetch_add(1, Ordering::SeqCst);
    }

    fn search(&self, _query: &FileQuery) -> ResultStream<'_> {
        Box::pin(async_stream::stream! {
            yield Ok(FileResult::new("counting", vec![Address::new("https", "https://x/f")]));
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
async fn capability_cache_is_opt_in_and_avoids_reprobing() {
    // Without the cache, capabilities are computed once per search.
    let (provider, calls, _) = Counting::new();
    let engine = Engine::new(vec![Arc::new(provider) as Arc<dyn SearchProvider>]);
    engine.search(FileQuery::new("x")).await;
    engine.search(FileQuery::new("x")).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    // With the cache, the second search reuses the first answer.
    let (provider, calls, _) = Counting::new();
    let engine =
        Engine::new(vec![Arc::new(provider) as Arc<dyn SearchProvider>]).with_capability_cache();
    engine.search(FileQuery::new("x")).await;
    engine.search(FileQuery::new("x")).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn search_stream_also_uses_the_capability_cache() {
    let (provider, calls, _) = Counting::new();
    let engine =
        Engine::new(vec![Arc::new(provider) as Arc<dyn SearchProvider>]).with_capability_cache();

    let mut stream = Box::pin(engine.search_stream(FileQuery::new("x")).await);
    while stream.next().await.is_some() {}
    let mut stream = Box::pin(engine.search_stream(FileQuery::new("x")).await);
    while stream.next().await.is_some() {}

    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn invalidate_capabilities_reprobes_and_delegates_to_providers() {
    let (provider, calls, invalidations) = Counting::new();
    let engine =
        Engine::new(vec![Arc::new(provider) as Arc<dyn SearchProvider>]).with_capability_cache();

    engine.search(FileQuery::new("x")).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    engine.invalidate_capabilities();
    assert_eq!(invalidations.load(Ordering::SeqCst), 1);

    engine.search(FileQuery::new("x")).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn probe_uses_the_capability_cache() {
    let (provider, calls, _) = Counting::new();
    let engine =
        Engine::new(vec![Arc::new(provider) as Arc<dyn SearchProvider>]).with_capability_cache();

    engine.probe(FileQuery::new("x")).await;
    engine.probe(FileQuery::new("x")).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
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
async fn engine_passes_paging_selectors_to_providers() {
    let results = (0..5)
        .map(|i| {
            let mut result = FileResult::new(
                "paged",
                vec![Address::new("https", format!("https://paged/file-{i}"))],
            );
            result.hashes = Some(HashMap::from([("sha256".to_owned(), format!("{i:064x}"))]));
            result
        })
        .collect();
    let engine = Engine::new(vec![Arc::new(Paged { results }) as Arc<dyn SearchProvider>]);

    // Page one: the provider skips 0 and returns 2.
    let first = engine
        .search(FileQuery::new("x").with_offset(0).with_limit(2))
        .await;
    let mut first_uris: Vec<&str> = first
        .results
        .iter()
        .map(|result| result.addresses[0].uri.as_str())
        .collect();
    first_uris.sort();
    assert_eq!(
        first_uris,
        vec!["https://paged/file-0", "https://paged/file-1"]
    );

    // Page two: the same query with an offset returns the next slice, without
    // the previous page having to be fetched and discarded client-side.
    let second = engine
        .search(FileQuery::new("x").with_offset(2).with_limit(2))
        .await;
    let mut second_uris: Vec<&str> = second
        .results
        .iter()
        .map(|result| result.addresses[0].uri.as_str())
        .collect();
    second_uris.sort();
    assert_eq!(
        second_uris,
        vec!["https://paged/file-2", "https://paged/file-3"]
    );
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

#[tokio::test]
async fn engine_reports_per_provider_metrics() {
    let providers: Vec<Arc<dyn SearchProvider>> = vec![
        Arc::new(Mock {
            result: result("a", "1111"),
        }),
        Arc::new(Mock {
            result: result("b", "2222"),
        }),
        Arc::new(Failing),
    ];
    let engine = Engine::new(providers);
    let report = engine.search(FileQuery::new("x")).await;

    let metrics = report.metrics;
    assert_eq!(metrics.len(), 3);
    // Sorted by provider name: a, b, failing.
    assert_eq!(metrics[0].provider, "a");
    assert_eq!(metrics[0].results, 1);
    assert_eq!(metrics[0].failures, 0);
    assert_eq!(metrics[1].provider, "b");
    assert_eq!(metrics[1].results, 1);
    assert_eq!(metrics[2].provider, "failing");
    assert_eq!(metrics[2].results, 0);
    assert_eq!(metrics[2].failures, 1);
}

#[tokio::test]
async fn engine_streams_outcomes_and_reports_skipped() {
    let providers: Vec<Arc<dyn SearchProvider>> = vec![
        Arc::new(Mock {
            result: result("a", "1111"),
        }),
        Arc::new(HashOnly),
    ];
    let engine = Engine::new(providers);

    let stream = engine.search_stream(FileQuery::new("x")).await;
    assert_eq!(stream.skipped, vec!["hash-only"]);

    let mut stream = Box::pin(stream);
    let mut results = 0;
    while let Some(outcome) = stream.next().await {
        if outcome.as_result().is_some() {
            results += 1;
        }
    }
    assert_eq!(results, 1);
}
