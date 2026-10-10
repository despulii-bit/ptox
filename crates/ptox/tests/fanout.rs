use futures::StreamExt;
use ptox::{
    fanout, fanout_with_metrics, Address, ErrorCode, FileQuery, FileResult, Metrics, Options,
    Outcome, ProviderError, ResultStream, SearchProvider,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn query() -> FileQuery {
    FileQuery::new("test query")
}

fn make_result(provider: &str, title: &str) -> FileResult {
    FileResult::new(
        provider,
        vec![Address::new(
            "https",
            format!("https://example.org/{title}"),
        )],
    )
}

struct Mock {
    name: String,
    results: Vec<FileResult>,
    error: Option<ProviderError>,
    delay: Option<Duration>,
    infinite: bool,
    drop_flag: Option<Arc<AtomicBool>>,
}

impl Mock {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            results: Vec::new(),
            error: None,
            delay: None,
            infinite: false,
            drop_flag: None,
        }
    }

    fn with_results(mut self, count: usize) -> Self {
        self.results = (0..count)
            .map(|i| make_result(&self.name, &format!("file-{i}")))
            .collect();
        self
    }

    fn with_error(mut self, code: &str) -> Self {
        self.error = Some(ProviderError::new(code, "boom"));
        self
    }

    fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }

    fn infinite(mut self) -> Self {
        self.infinite = true;
        self
    }

    fn with_drop_flag(mut self, flag: Arc<AtomicBool>) -> Self {
        self.drop_flag = Some(flag);
        self
    }
}

impl SearchProvider for Mock {
    fn name(&self) -> &str {
        &self.name
    }

    fn search(&self, _query: &FileQuery) -> ResultStream<'_> {
        let name = self.name.clone();
        let results = self.results.clone();
        let error = self.error.clone();
        let delay = self.delay;
        let infinite = self.infinite;
        let drop_flag = self.drop_flag.clone();

        Box::pin(async_stream::stream! {
            let _guard = drop_flag.map(DropGuard);
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            if let Some(error) = error {
                yield Err(error);
                return;
            }
            if infinite {
                loop {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                    yield Ok(make_result(&name, "spam"));
                }
            }
            for result in results {
                yield Ok(result);
            }
        })
    }
}

struct DropGuard(Arc<AtomicBool>);

impl Drop for DropGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn providers(list: Vec<Mock>) -> Vec<Arc<dyn SearchProvider>> {
    list.into_iter()
        .map(|m| Arc::new(m) as Arc<dyn SearchProvider>)
        .collect()
}

async fn collect(stream: impl futures::Stream<Item = Outcome>) -> Vec<Outcome> {
    futures::pin_mut!(stream);
    let mut outcomes = Vec::new();
    while let Some(outcome) = stream.next().await {
        outcomes.push(outcome);
    }
    outcomes
}

#[tokio::test]
async fn one_provider_failing_does_not_affect_others() {
    let outcomes = collect(fanout(
        query(),
        providers(vec![
            Mock::new("a").with_results(2),
            Mock::new("bad").with_error("refused"),
            Mock::new("b").with_results(1),
        ]),
        Options::default(),
    ))
    .await;

    let results = outcomes.iter().filter(|o| o.as_result().is_some()).count();
    let failures = outcomes.iter().filter(|o| o.as_failure().is_some()).count();
    assert_eq!(results, 3);
    assert_eq!(failures, 1);
}

#[tokio::test]
async fn slow_provider_yields_timeout_failure() {
    let options = Options {
        timeout: Some(Duration::from_millis(100)),
        ..Options::default()
    };
    let outcomes = collect(fanout(
        query(),
        providers(vec![Mock::new("slow")
            .with_delay(Duration::from_secs(5))
            .with_results(1)]),
        options,
    ))
    .await;

    assert_eq!(outcomes.len(), 1);
    match &outcomes[0] {
        Outcome::Failure(failure) => assert_eq!(failure.error.code, ErrorCode::timeout()),
        other => panic!("expected a timeout failure, got {other:?}"),
    }
}

#[tokio::test]
async fn fast_results_stream_before_slow_provider_finishes() {
    let mut stream = Box::pin(fanout(
        query(),
        providers(vec![
            Mock::new("fast").with_results(1),
            Mock::new("slow")
                .with_delay(Duration::from_secs(2))
                .with_results(1),
        ]),
        Options::default(),
    ));

    let first = tokio::time::timeout(Duration::from_millis(300), stream.next())
        .await
        .expect("a result should stream before the slow provider finishes")
        .expect("stream should not be empty");

    match first {
        Outcome::Result(result) => assert_eq!(result.provider, "fast"),
        other => panic!("expected the fast result first, got {other:?}"),
    }
}

#[tokio::test]
async fn dropping_the_stream_cancels_providers() {
    let flag = Arc::new(AtomicBool::new(false));
    let mut stream = Box::pin(fanout(
        query(),
        providers(vec![Mock::new("inf")
            .infinite()
            .with_drop_flag(flag.clone())]),
        Options::default(),
    ));

    let _ = stream.next().await;
    drop(stream);

    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(
        flag.load(Ordering::SeqCst),
        "provider stream was not dropped after cancellation"
    );
}

#[tokio::test]
async fn global_limit_caps_streamed_results() {
    let options = Options {
        limit: Some(3),
        ..Options::default()
    };
    let outcomes = collect(fanout(
        query(),
        providers(vec![Mock::new("p").with_results(100)]),
        options,
    ))
    .await;

    assert_eq!(
        outcomes.iter().filter(|o| o.as_result().is_some()).count(),
        3
    );
}

#[tokio::test]
async fn global_offset_skips_leading_results_then_limit_applies() {
    let options = Options {
        offset: Some(3),
        limit: Some(2),
        ..Options::default()
    };
    let outcomes = collect(fanout(
        query(),
        providers(vec![Mock::new("p").with_results(10)]),
        options,
    ))
    .await;

    let uris: Vec<&str> = outcomes
        .iter()
        .filter_map(|outcome| outcome.as_result())
        .map(|result| result.addresses[0].uri.as_str())
        .collect();
    assert_eq!(
        uris,
        vec!["https://example.org/file-3", "https://example.org/file-4"]
    );
}

#[tokio::test]
async fn global_offset_without_limit_yields_the_tail() {
    let options = Options {
        offset: Some(7),
        ..Options::default()
    };
    let outcomes = collect(fanout(
        query(),
        providers(vec![Mock::new("p").with_results(10)]),
        options,
    ))
    .await;

    assert_eq!(
        outcomes.iter().filter(|o| o.as_result().is_some()).count(),
        3
    );
}

#[tokio::test]
async fn global_offset_never_skips_failures() {
    let options = Options {
        offset: Some(5),
        ..Options::default()
    };
    let outcomes = collect(fanout(
        query(),
        providers(vec![Mock::new("p").with_error("transport")]),
        options,
    ))
    .await;

    assert_eq!(outcomes.len(), 1);
    assert!(outcomes[0].as_failure().is_some());
}

#[tokio::test]
async fn results_are_attributed_and_shaped() {
    let outcomes = collect(fanout(
        query(),
        providers(vec![Mock::new("x").with_results(2)]),
        Options::default(),
    ))
    .await;

    assert_eq!(outcomes.len(), 2);
    for outcome in &outcomes {
        let result = outcome.as_result().expect("expected a result");
        assert!(!result.provider.is_empty());
        assert!(!result.addresses.is_empty());
    }
}

/// A provider that forgets to set `provider` (as wire providers may).
struct Anonymous;

impl SearchProvider for Anonymous {
    fn name(&self) -> &str {
        "anon"
    }

    fn search(&self, _query: &FileQuery) -> ResultStream<'_> {
        Box::pin(async_stream::stream! {
            yield Ok(FileResult::new(
                "",
                vec![Address::new("https", "https://example.org/x")],
            ));
        })
    }
}

#[tokio::test]
async fn fanout_attributes_anonymous_results() {
    let providers: Vec<Arc<dyn SearchProvider>> = vec![Arc::new(Anonymous)];
    let outcomes = collect(fanout(query(), providers, Options::default())).await;

    let result = outcomes
        .iter()
        .find_map(|outcome| outcome.as_result())
        .expect("a result");
    assert_eq!(result.provider, "anon");
}

/// A provider that emits results exactly as given, without going through
/// `FileResult::new`'s shape guarantees.
struct Raw {
    name: &'static str,
    results: Vec<FileResult>,
}

impl SearchProvider for Raw {
    fn name(&self) -> &str {
        self.name
    }

    fn search(&self, _query: &FileQuery) -> ResultStream<'_> {
        let results = self.results.clone();
        Box::pin(async_stream::stream! {
            for result in results {
                yield Ok(result);
            }
        })
    }
}

#[tokio::test]
async fn structurally_invalid_results_become_malformed_failures() {
    let metrics = Metrics::new();
    // A result with no address, and one with an address that has an empty URI.
    let no_address = FileResult::new("p", Vec::new());
    let empty_uri = FileResult::new("p", vec![Address::new("https", "")]);
    let valid = make_result("p", "good");
    let provider: Vec<Arc<dyn SearchProvider>> = vec![Arc::new(Raw {
        name: "p",
        results: vec![no_address, empty_uri, valid],
    })];

    let outcomes = collect(fanout_with_metrics(
        query(),
        provider,
        Options::default(),
        &metrics,
    ))
    .await;

    // Only the well-formed result reaches the client...
    let results: Vec<&FileResult> = outcomes
        .iter()
        .filter_map(|outcome| outcome.as_result())
        .collect();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].addresses[0].uri, "https://example.org/good");

    // ...and the two invalid results are reported as `malformed` failures.
    let failures: Vec<&ptox::Failure> = outcomes
        .iter()
        .filter_map(|outcome| outcome.as_failure())
        .collect();
    assert_eq!(failures.len(), 2);
    for failure in &failures {
        assert_eq!(failure.provider, "p");
        assert_eq!(failure.error.code, ErrorCode::malformed());
    }

    let snapshot = metrics.snapshot();
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].results, 1);
    assert_eq!(snapshot[0].failures, 2);
}

#[tokio::test]
async fn provider_with_only_invalid_results_yields_no_results() {
    let provider: Vec<Arc<dyn SearchProvider>> = vec![Arc::new(Raw {
        name: "bad",
        results: vec![FileResult::new("bad", Vec::new())],
    })];
    let outcomes = collect(fanout(query(), provider, Options::default())).await;

    assert!(outcomes.iter().all(|outcome| outcome.as_result().is_none()));
    assert_eq!(outcomes.len(), 1);
    assert_eq!(
        outcomes[0].as_failure().map(|failure| &failure.error.code),
        Some(&ErrorCode::malformed())
    );
}

#[tokio::test]
async fn core_never_fetches_addresses() {
    // The core has no fetch path: an address is returned untouched, never resolved.
    let outcomes = collect(fanout(
        query(),
        providers(vec![Mock::new("x").with_results(1)]),
        Options::default(),
    ))
    .await;

    let result = outcomes
        .iter()
        .find_map(|o| o.as_result())
        .expect("a result");
    assert_eq!(result.addresses[0].uri, "https://example.org/file-0");
}

#[tokio::test]
async fn metrics_record_result_counts_and_elapsed() {
    let metrics = Metrics::new();
    let outcomes = collect(fanout_with_metrics(
        query(),
        providers(vec![
            Mock::new("a").with_results(3),
            Mock::new("b").with_results(1),
        ]),
        Options::default(),
        &metrics,
    ))
    .await;

    assert_eq!(
        outcomes.iter().filter(|o| o.as_result().is_some()).count(),
        4
    );
    let snapshot = metrics.snapshot();
    assert_eq!(snapshot.len(), 2);
    // Sorted by provider name.
    assert_eq!(snapshot[0].provider, "a");
    assert_eq!(snapshot[0].results, 3);
    assert_eq!(snapshot[0].failures, 0);
    assert_eq!(snapshot[1].provider, "b");
    assert_eq!(snapshot[1].results, 1);
}

#[tokio::test]
async fn metrics_record_failures_and_timeouts() {
    let options = Options {
        timeout: Some(Duration::from_millis(100)),
        ..Options::default()
    };
    let metrics = Metrics::new();
    let _ = collect(fanout_with_metrics(
        query(),
        providers(vec![
            Mock::new("bad").with_error("refused"),
            Mock::new("slow")
                .with_delay(Duration::from_secs(5))
                .with_results(1),
        ]),
        options,
        &metrics,
    ))
    .await;

    let snapshot = metrics.snapshot();
    assert_eq!(snapshot.len(), 2);
    assert_eq!(snapshot[0].provider, "bad");
    assert_eq!(snapshot[0].failures, 1);
    assert_eq!(snapshot[1].provider, "slow");
    assert_eq!(snapshot[1].failures, 1);
    assert!(!metrics.is_empty());
}
