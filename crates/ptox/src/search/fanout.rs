use super::provider::SearchProvider;
use crate::types::{ErrorCode, ErrorInfo, Failure, FileQuery, Outcome, Validate};
use async_stream::stream;
use futures::{Stream, StreamExt};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Semaphore};

/// Optional policy knobs. The core has no required policy.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Total time budget per provider. Exceeding it yields a `timeout` failure.
    pub timeout: Option<Duration>,
    /// Total wall-clock budget for the whole fan-out, measured from when the
    /// stream is first polled.
    ///
    /// When it elapses, the stream stops and every provider still running (or
    /// not yet started) is reported as a single `timeout` failure, so a caller
    /// keeps the partial results already delivered instead of waiting for the
    /// sum of each provider's [`timeout`](Self::timeout). `None` (the default)
    /// means no overall budget.
    pub deadline: Option<Duration>,
    /// Global cap on the number of `Outcome::Result` items streamed.
    pub limit: Option<usize>,
    /// Global number of `Outcome::Result` items to skip before streaming, the
    /// pagination counterpart to [`limit`](Self::limit).
    ///
    /// Failures are never skipped, so health/diagnostic information is not lost
    /// by paging. Like `limit`, this is a policy knob over the raw arrival order,
    /// which the contract does not guarantee to be stable, so it is meaningful
    /// within one fan-out, not as a globally stable cursor.
    pub offset: Option<usize>,
    /// Maximum number of providers polled concurrently.
    pub concurrency: Option<usize>,
}

/// What one provider did during a fan-out: how many results and failures it
/// produced, and how long it took.
///
/// Metrics carry **counts and timings only** — never query text or result
/// content — so a client can show progress and diagnostics without leaking what
/// was searched for. A provider error or timeout ends that provider's stream, so
/// at most one of those is counted; each structurally invalid result the fan-out
/// rejects ([`fanout_with_metrics`]) is counted as an additional failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderMetric {
    /// The provider's stable name.
    pub provider: String,
    /// Number of `Outcome::Result` items it emitted.
    pub results: usize,
    /// Number of failures attributed to it: a provider error or timeout (at most
    /// one, since either ends the stream) plus one for each structurally invalid
    /// result the fan-out rejected.
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
///
/// The fan-out also enforces the structural contract (`ptox::types::Validate`)
/// at this boundary: a result missing an address, or carrying an empty address
/// scheme/URI, is **not** streamed to the client. Instead it is reported as a
/// `malformed` [`Outcome::Failure`] attributed to the emitting provider, and the
/// provider keeps streaming, so one bad entry never discards its good results.
/// Attribution is applied before validation, so a provider may omit `provider`
/// on a result.
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

        let names: Vec<String> = providers.iter().map(|p| p.name().to_owned()).collect();
        let finished: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));

        for provider in providers {
            let tx = tx.clone();
            let query = query.clone();
            let timeout = options.timeout;
            let semaphore = semaphore.clone();
            let metrics = metrics.clone();
            let finished = finished.clone();

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
                                // Enforce the structural contract at the fan-out
                                // boundary: a result the contract rejects (no
                                // address, or an empty scheme/uri) must never
                                // reach a client's merge/fetch path. Report it as
                                // a provider failure and keep consuming, so one
                                // bad entry doesn't discard the provider's good
                                // results.
                                if let Err(error) = result.validate() {
                                    failure_count += 1;
                                    let _ = tx
                                        .send(Outcome::Failure(Failure {
                                            provider: name.clone(),
                                            error: ErrorInfo {
                                                code: ErrorCode::malformed(),
                                                message: format!(
                                                    "provider returned an invalid result: {error}"
                                                ),
                                            },
                                        }))
                                        .await;
                                    continue;
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
                    provider: name.clone(),
                    results: result_count,
                    failures: failure_count,
                    elapsed: start.elapsed(),
                });
                if let Ok(mut finished) = finished.lock() {
                    finished.insert(name);
                }
            });
        }

        drop(tx);

        let deadline = options
            .deadline
            .map(|after| tokio::time::Instant::now() + after);
        let mut yielded = 0usize;
        let mut skipped = 0usize;
        let offset = options.offset.unwrap_or(0);
        let mut hit_deadline = false;
        loop {
            let next = match deadline {
                Some(at) => tokio::time::timeout_at(at, rx.recv()).await,
                None => Ok::<_, tokio::time::error::Elapsed>(rx.recv().await),
            };
            let outcome = match next {
                Ok(Some(outcome)) => outcome,
                Ok(None) => break,
                Err(_) => {
                    hit_deadline = true;
                    break;
                }
            };
            if matches!(outcome, Outcome::Result(_)) {
                if skipped < offset {
                    skipped += 1;
                    continue;
                }
                if options.limit.is_some_and(|limit| yielded >= limit) {
                    break;
                }
                yielded += 1;
            }
            yield outcome;
        }

        if hit_deadline {
            let done = finished.lock().map(|done| done.clone()).unwrap_or_default();
            let budget = options.deadline.unwrap_or_default();
            for name in names {
                if !done.contains(&name) {
                    yield Outcome::Failure(Failure {
                        provider: name,
                        error: ErrorInfo {
                            code: ErrorCode::timeout(),
                            message: format!(
                                "search deadline of {budget:?} exceeded before the provider finished"
                            ),
                        },
                    });
                }
            }
        }
    }
}
