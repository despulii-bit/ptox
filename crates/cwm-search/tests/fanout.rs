use cwm_search::{fanout, Options, ProviderError, ResultStream, SearchProvider};
use cwm_types::{Address, ErrorCode, FileQuery, FileResult, Outcome};
use futures::StreamExt;
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
