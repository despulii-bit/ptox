//! Provider health probing: ask each provider one query and classify its answer.

use crate::search::SearchProvider;
use crate::types::{ErrorInfo, FileQuery};
use futures::StreamExt;
use std::sync::Arc;
use std::time::Duration;

/// How a provider responded to a probe query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeStatus {
    /// Answered with one or more results.
    Results(usize),
    /// Answered cleanly with no results.
    Empty,
    /// Declared capabilities reject the probe query, so it was not asked.
    Skipped,
    /// Returned an error.
    Failed(ErrorInfo),
    /// Did not finish within the timeout.
    Timeout,
}

impl ProbeStatus {
    /// Whether the provider is unusable for this probe: it failed or timed out.
    pub fn is_failure(&self) -> bool {
        matches!(self, ProbeStatus::Failed(_) | ProbeStatus::Timeout)
    }
}

/// The probe outcome for one provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderProbe {
    /// The provider's name.
    pub provider: String,
    /// What happened when it was asked.
    pub status: ProbeStatus,
}

/// Probe each provider with `query`, concurrently, respecting declared
/// capabilities.
///
/// A provider whose capabilities reject the query is reported as
/// [`ProbeStatus::Skipped`] without being asked. A provider that streams past
/// `timeout` is reported as [`ProbeStatus::Timeout`].
pub async fn probe(
    query: FileQuery,
    providers: Vec<Arc<dyn SearchProvider>>,
    timeout: Duration,
) -> Vec<ProviderProbe> {
    let checks = providers.into_iter().map(|provider| {
        let probe = query.clone();
        async move {
            let name = provider.name().to_owned();

            if !provider.capabilities().await.accepts(&probe) {
                return ProviderProbe {
                    provider: name,
                    status: ProbeStatus::Skipped,
                };
            }

            let mut stream = provider.search(&probe);
            let mut count = 0usize;
            let mut failure = None;
            let finished = tokio::time::timeout(timeout, async {
                while let Some(item) = stream.next().await {
                    match item {
                        Ok(_) => count += 1,
                        Err(error) => {
                            failure = Some(error);
                            break;
                        }
                    }
                }
            })
            .await;

            let status = if finished.is_err() {
                ProbeStatus::Timeout
            } else if let Some(error) = failure {
                ProbeStatus::Failed(ErrorInfo {
                    code: error.code,
                    message: error.message,
                })
            } else if count == 0 {
                ProbeStatus::Empty
            } else {
                ProbeStatus::Results(count)
            };

            ProviderProbe {
                provider: name,
                status,
            }
        }
    });

    futures::future::join_all(checks).await
}
