use async_trait::async_trait;
use ptox::{
    probe, provider_capabilities, Address, Capabilities, ErrorCode, FileQuery, FileResult,
    ProbeStatus, ProviderError, ResultStream, SearchProvider,
};
use std::sync::Arc;
use std::time::Duration;

fn result(name: &str) -> FileResult {
    FileResult::new(name, vec![Address::new("https", "https://example.org/x")])
}

/// Yields `count` results, then ends.
struct Static {
    name: &'static str,
    count: usize,
}

impl SearchProvider for Static {
    fn name(&self) -> &str {
        self.name
    }

    fn search(&self, _query: &FileQuery) -> ResultStream<'_> {
        let name = self.name.to_owned();
        let count = self.count;
        Box::pin(async_stream::stream! {
            for _ in 0..count {
                yield Ok(result(&name));
            }
        })
    }
}

/// Fails on the first item.
struct Failing;

impl SearchProvider for Failing {
    fn name(&self) -> &str {
        "failing"
    }

    fn search(&self, _query: &FileQuery) -> ResultStream<'_> {
        Box::pin(
            async_stream::stream! { yield Err(ProviderError::new(ErrorCode::refused(), "nope")); },
        )
    }
}

/// Never finishes.
struct Hanging;

impl SearchProvider for Hanging {
    fn name(&self) -> &str {
        "hanging"
    }

    fn search(&self, _query: &FileQuery) -> ResultStream<'_> {
        Box::pin(async_stream::stream! {
            tokio::time::sleep(Duration::from_secs(3600)).await;
            yield Ok(result("hanging"));
        })
    }
}

/// Accepts hash queries only.
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
        Box::pin(async_stream::stream! { yield Ok(result("hash-only")); })
    }
}

/// Declares capabilities only after `delay`.
struct SlowCapabilities {
    delay: Duration,
}

#[async_trait]
impl SearchProvider for SlowCapabilities {
    fn name(&self) -> &str {
        "slow-capabilities"
    }

    async fn capabilities(&self) -> Capabilities {
        tokio::time::sleep(self.delay).await;
        Capabilities {
            query: vec!["text".to_owned()],
            ..Capabilities::default()
        }
    }

    fn search(&self, _query: &FileQuery) -> ResultStream<'_> {
        Box::pin(async_stream::stream! { yield Ok(result("slow-capabilities")); })
    }
}

#[tokio::test]
async fn probe_classifies_every_provider() {
    let providers: Vec<Arc<dyn SearchProvider>> = vec![
        Arc::new(Static {
            name: "good",
            count: 2,
        }),
        Arc::new(Static {
            name: "empty",
            count: 0,
        }),
        Arc::new(Failing),
        Arc::new(HashOnly),
    ];

    let reports = probe(FileQuery::new("text"), providers, Duration::from_secs(5)).await;
    let status = |name: &str| {
        reports
            .iter()
            .find(|report| report.provider == name)
            .expect("provider present")
            .status
            .clone()
    };

    assert_eq!(status("good"), ProbeStatus::Results(2));
    assert_eq!(status("empty"), ProbeStatus::Empty);
    assert!(matches!(status("failing"), ProbeStatus::Failed(_)));
    assert_eq!(status("hash-only"), ProbeStatus::Skipped);
}

#[tokio::test]
async fn probe_reports_a_timeout() {
    let providers: Vec<Arc<dyn SearchProvider>> = vec![Arc::new(Hanging)];
    let reports = probe(
        FileQuery::new("text"),
        providers,
        Duration::from_millis(100),
    )
    .await;

    assert_eq!(reports[0].status, ProbeStatus::Timeout);
    assert!(reports[0].status.is_failure());
}

#[tokio::test]
async fn provider_capabilities_lists_declarations_without_a_query() {
    let providers: Vec<Arc<dyn SearchProvider>> = vec![
        Arc::new(Static {
            name: "universal",
            count: 1,
        }),
        Arc::new(HashOnly),
    ];

    let reports = provider_capabilities(providers, Duration::from_secs(5)).await;

    assert_eq!(reports.len(), 2);
    // A provider with no declaration reports the universal default.
    assert_eq!(reports[0].provider, "universal");
    assert!(reports[0].answered);
    assert_eq!(reports[0].capabilities, Capabilities::universal());
    // The declared facets come back intact.
    assert_eq!(reports[1].provider, "hash-only");
    assert!(reports[1].answered);
    assert_eq!(reports[1].capabilities.query, vec!["hashes"]);
    assert_eq!(reports[1].capabilities.requires, vec!["hashes"]);
    assert_eq!(reports[1].capabilities.schemes, vec!["magnet"]);
}

#[tokio::test]
async fn provider_capabilities_falls_back_to_universal_on_timeout() {
    let providers: Vec<Arc<dyn SearchProvider>> = vec![Arc::new(SlowCapabilities {
        delay: Duration::from_secs(3600),
    })];

    let reports = provider_capabilities(providers, Duration::from_millis(50)).await;

    assert_eq!(reports[0].provider, "slow-capabilities");
    assert!(!reports[0].answered);
    assert_eq!(reports[0].capabilities, Capabilities::universal());
}

/// Reports `unavailable` — e.g. a daemon still bootstrapping.
struct Unavailable;

impl SearchProvider for Unavailable {
    fn name(&self) -> &str {
        "warming"
    }

    fn search(&self, _query: &FileQuery) -> ResultStream<'_> {
        Box::pin(async_stream::stream! {
            yield Err(ProviderError::new(ErrorCode::unavailable(), "daemon bootstrapping"));
        })
    }
}

#[tokio::test]
async fn probe_reports_pending_when_a_provider_is_not_ready() {
    let providers: Vec<Arc<dyn SearchProvider>> = vec![Arc::new(Unavailable)];
    let reports = probe(FileQuery::new("text"), providers, Duration::from_secs(5)).await;

    assert_eq!(reports[0].status, ProbeStatus::Pending);
    assert!(!reports[0].status.is_failure());
}
