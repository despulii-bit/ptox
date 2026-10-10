use super::provider::SearchProvider;
use crate::types::{ErrorCode, ErrorInfo, Failure, FileQuery, Outcome};
use async_stream::stream;
use futures::{Stream, StreamExt};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Semaphore};

/// Optional policy knobs. The core has no required policy.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Total time budget per provider. Exceeding it yields a `timeout` failure.
    pub timeout: Option<Duration>,
    /// Global cap on the number of `Outcome::Result` items streamed.
    pub limit: Option<usize>,
    /// Maximum number of providers polled concurrently.
    pub concurrency: Option<usize>,
}

/// What one provider did during a fan-out: how many results and failures it
/// produced, and how long it took.
///
/// Metrics carry **counts and timings only** — never query text or result
/// content — so a client can show progress and diagnostics without leaking what
/// was searched for. A provider stops at its first error or timeout, so
/// `failures` is at most one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderMetric {
    /// The provider's stable name.
    pub provider: String,
    /// Number of `Outcome::Result` items it emitted.
    pub results: usize,
    /// Number of failures it reported (`timeout` included); at most one.
    pub failures: usize,
    /// Wall-clock time from the start of its search to its completion,
    /// including time spent waiting to acquire a concurrency permit.
    pub elapsed: Duration,
}

/// A shared collector for [`ProviderMetric`]s, filled by
/// [`fanout_with_metrics`].
///
/// Clone it to share one collector with the fan-out; [`Metrics::snapshot`] reads
/// what has been recorded so far, sorted by provider name. Recording and
/// snapshotting are cheap and safe to call from multiple tasks.
#[derive(Clone, Default)]
pub struct Metrics {
    entries: Arc<Mutex<Vec<ProviderMetric>>>,
}

impl Metrics {
    /// An empty collector.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one provider's metrics. Called by [`fanout_with_metrics`].
    pub fn record(&self, metric: ProviderMetric) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.push(metric);
        }
    }

    /// The metrics recorded so far, sorted by provider name for determinism.
    pub fn snapshot(&self) -> Vec<ProviderMetric> {
        let mut entries = self
            .entries
            .lock()
            .map(|entries| entries.clone())
            .unwrap_or_default();
        entries.sort_by(|a, b| a.provider.cmp(&b.provider));
        entries
    }

    /// Whether any metrics have been recorded.
    pub fn is_empty(&self) -> bool {
        self.entries
            .lock()
            .map(|entries| entries.is_empty())
            .unwrap_or(true)
    }
}

/// Fan a query out to all providers and stream attributed outcomes as they
/// arrive. One provider failing, timing out, or hanging never affects the others.
///
/// Attribution is normalized here: a result with an empty `provider` is stamped
/// with the emitting provider's name, so Rust provider authors (like wire
/// providers) never have to set it themselves.
///
/// Dropping the returned stream cancels the in-flight providers.
///
/// This is [`fanout_with_metrics`] with a throwaway collector; use that function
/// when you want per-provider counts and timings.
pub fn fanout(
    query: FileQuery,
    providers: Vec<Arc<dyn SearchProvider>>,
    options: Options,
) -> impl Stream<Item = Outcome> + 'static {
    fanout_with_metrics(query, providers, options, &Metrics::new())
}

/// Like [`fanout`], but record one [`ProviderMetric`] per provider into
/// `metrics` as it finishes.
///
/// A provider that finishes (or errors, or times out) records its metrics. If
/// the returned stream is dropped early, an in-flight provider records whatever
/// it streamed before its next send fails.
pub fn fanout_with_metrics(
    query: FileQuery,
    providers: Vec<Arc<dyn SearchProvider>>,
    options: Options,
    metrics: &Metrics,
) -> impl Stream<Item = Outcome> + 'static {
    let metrics = metrics.clone();
    stream! {
        let (tx, mut rx) = mpsc::channel::<Outcome>(64);
        let concurrency = options
            .concurrency
            .unwrap_or(Semaphore::MAX_PERMITS)
            .max(1);
        let semaphore = Arc::new(Semaphore::new(concurrency));

        for provider in providers {
            let tx = tx.clone();
            let query = query.clone();
            let timeout = options.timeout;
            let semaphore = semaphore.clone();
            let metrics = metrics.clone();

            tokio::spawn(async move {
                let start = Instant::now();
                let _permit = semaphore.acquire_owned().await;
                let name = provider.name().to_string();
                let mut result_count = 0usize;
                let mut failure_count = 0usize;
                let mut results = provider.search(&query);

                let forward = async {
                    while let Some(item) = results.next().await {
                        let outcome = match item {
                            Ok(mut result) => {
                                // The host owns attribution (contract §9).
                                if result.provider.is_empty() {
                                    result.provider = name.clone();
                                }
                                result_count += 1;
                                Outcome::Result(result)
                            }
                            Err(error) => {
                                failure_count += 1;
                                let _ = tx
                                    .send(Outcome::Failure(Failure {
                                        provider: name.clone(),
                                        error: error.into(),
                                    }))
                                    .await;
                                return;
                            }
                        };
                        if tx.send(outcome).await.is_err() {
                            return;
                        }
                    }
                };

                if let Some(duration) = timeout {
                    if tokio::time::timeout(duration, forward).await.is_err() {
                        failure_count += 1;
                        let _ = tx
                            .send(Outcome::Failure(Failure {
                                provider: name.clone(),
                                error: ErrorInfo {
                                    code: ErrorCode::timeout(),
                                    message: format!("provider timed out after {duration:?}"),
                                },
                            }))
                            .await;
                    }
                } else {
                    forward.await;
                }

                metrics.record(ProviderMetric {
                    provider: name,
                    results: result_count,
                    failures: failure_count,
                    elapsed: start.elapsed(),
                });
            });
        }

        drop(tx);

        let mut yielded = 0usize;
        while let Some(outcome) = rx.recv().await {
            if matches!(outcome, Outcome::Result(_)) {
                if options.limit.is_some_and(|limit| yielded >= limit) {
                    break;
                }
                yielded += 1;
            }
            yield outcome;
        }
    }
}
