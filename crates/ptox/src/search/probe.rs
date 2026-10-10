//! Provider health probing: ask each provider one query and classify its answer.

use crate::search::{Capabilities, SearchProvider};
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

/// One provider's declared [`Capabilities`], as returned by
/// [`provider_capabilities`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCapabilities {
    /// The provider's name.
    pub provider: String,
    /// What the provider declared, or [`Capabilities::universal`] if it did not
    /// answer before the timeout.
    pub capabilities: Capabilities,
    /// Whether the provider answered in time. `false` means `capabilities` is
    /// the universal fallback rather than a declaration.
    pub answered: bool,
}

/// Ask each provider for its declared [`Capabilities`], concurrently, without
/// running a query.
///
/// This is the query-free counterpart to [`probe`]: a `providers` or
/// diagnostics command can list what each provider supports (query facets,
/// requirements, and address schemes) before sending anything. A provider that
/// does not answer within `timeout` is reported with
/// [`Capabilities::universal`] and `answered: false`; the function never fails,
/// so there is always a row per provider.
///
/// Results are in the order the providers were supplied, and an engine's
/// [`crate::Engine::with_capability_cache`] memo is reused when enabled.
pub async fn provider_capabilities(
    providers: Vec<Arc<dyn SearchProvider>>,
    timeout: Duration,
) -> Vec<ProviderCapabilities> {
    let checks = providers.into_iter().map(|provider| async move {
        let name = provider.name().to_owned();
        match tokio::time::timeout(timeout, provider.capabilities()).await {
            Ok(capabilities) => ProviderCapabilities {
                provider: name,
                capabilities,
                answered: true,
            },
            Err(_) => ProviderCapabilities {
                provider: name,
                capabilities: Capabilities::universal(),
                answered: false,
            },
        }
    });

    futures::future::join_all(checks).await
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
