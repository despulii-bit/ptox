use super::provider::SearchProvider;
use crate::types::{ErrorCode, ErrorInfo, Failure, FileQuery, Outcome};
use async_stream::stream;
use futures::{Stream, StreamExt};
use std::sync::Arc;
use std::time::Duration;
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

/// Fan a query out to all providers and stream attributed outcomes as they
/// arrive. One provider failing, timing out, or hanging never affects the others.
///
/// Dropping the returned stream cancels the in-flight providers.
pub fn fanout(
    query: FileQuery,
    providers: Vec<Arc<dyn SearchProvider>>,
    options: Options,
) -> impl Stream<Item = Outcome> + 'static {
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

            tokio::spawn(async move {
                let _permit = semaphore.acquire_owned().await;
                let name = provider.name().to_string();
                let mut results = provider.search(&query);

                let forward = async {
                    while let Some(item) = results.next().await {
                        let outcome = match item {
                            Ok(result) => Outcome::Result(result),
                            Err(error) => {
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
