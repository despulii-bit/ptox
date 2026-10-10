//! The high-level entry point: providers in, ranked results and fetched files out.
//!
//! [`Engine`] bundles the search pipeline ([`crate::fanout`] + [`crate::merge`]) and,
//! with the `fetch` feature, retrieval. A client is then a few lines:
//!
//! ```no_run
//! # async fn run(providers: Vec<std::sync::Arc<dyn ptox::SearchProvider>>) {
//! let engine = ptox::Engine::new(providers);
//! let report = engine.search(ptox::FileQuery::new("annual report")).await;
//! for result in &report.results {
//!     println!("{:?} from {:?}", result.title, result.providers);
//! }
//! # }
//! ```

use crate::merge::{merge, MergeOptions, MergedResult};
use crate::search::{fanout, Options, ProviderProbe, SearchProvider};
use crate::types::{Failure, FileQuery, Outcome};
use futures::{Stream, StreamExt};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "fetch")]
use crate::fetch::{FetchError, FetchRegistry, Fetched, ProgressFn};
#[cfg(feature = "fetch")]
use std::path::PathBuf;

/// The outcome of one search: ranked results plus per-provider failures.
pub struct SearchReport {
    /// Ranked, deduplicated hits across every provider that answered.
    pub results: Vec<MergedResult>,
    /// One entry per provider that failed; never aborts the search.
    pub failures: Vec<Failure>,
    /// Providers skipped because their declared capabilities reject the query.
    pub skipped: Vec<String>,
}

/// A streaming search: which providers were skipped, plus the live outcome stream.
///
/// A [`SearchStream`] is itself a [`Stream`] of [`Outcome`]s, so it can be
/// consumed directly. Dropping it cancels in-flight providers.
pub struct SearchStream {
    /// Providers skipped because their declared capabilities reject the query.
    pub skipped: Vec<String>,
    /// The live outcomes, as they arrive (never merged or ranked).
    outcomes: Pin<Box<dyn Stream<Item = Outcome> + Send>>,
}

impl SearchStream {
    /// Take the underlying outcome stream, discarding the skipped list.
    pub fn into_outcomes(self) -> Pin<Box<dyn Stream<Item = Outcome> + Send>> {
        self.outcomes
    }
}

impl Stream for SearchStream {
    type Item = Outcome;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.outcomes.as_mut().poll_next(cx)
    }
}

/// Runs providers and (optionally) retrieves results.
pub struct Engine {
    providers: Vec<Arc<dyn SearchProvider>>,
    options: Options,
    provider_weights: HashMap<String, f64>,
    #[cfg(feature = "fetch")]
    fetchers: FetchRegistry,
}

impl Engine {
    /// An engine over `providers` with default search options (and, with the
    /// `fetch` feature, the default fetchers).
    #[cfg(feature = "fetch")]
    pub fn new(providers: Vec<Arc<dyn SearchProvider>>) -> Self {
        Self {
            providers,
            options: Options::default(),
            provider_weights: HashMap::new(),
            fetchers: FetchRegistry::with_defaults(),
        }
    }

    /// An engine over `providers` with default search options.
    #[cfg(not(feature = "fetch"))]
    pub fn new(providers: Vec<Arc<dyn SearchProvider>>) -> Self {
        Self {
            providers,
            options: Options::default(),
            provider_weights: HashMap::new(),
        }
    }

    /// Override the [`Options`] used for searches.
    pub fn with_options(mut self, options: Options) -> Self {
        self.options = options;
        self
    }

    /// Set per-provider ranking weights, as consumed by [`crate::merge`].
    pub fn with_weights(mut self, provider_weights: HashMap<String, f64>) -> Self {
        self.provider_weights = provider_weights;
        self
    }

    /// Replace the retrieval registry used by [`Engine::fetch`].
    #[cfg(feature = "fetch")]
    pub fn with_fetchers(mut self, fetchers: FetchRegistry) -> Self {
        self.fetchers = fetchers;
        self
    }

    /// The providers this engine was built with.
    pub fn providers(&self) -> &[Arc<dyn SearchProvider>] {
        &self.providers
    }

    /// Run the query across every capable provider; return ranked, deduplicated
    /// results. Providers whose declared capabilities reject the query are
    /// skipped (listed in [`SearchReport::skipped`]) without being spawned.
    pub async fn search(&self, query: FileQuery) -> SearchReport {
        let SearchStream { skipped, outcomes } = self.search_stream(query).await;
        let outcomes = collect(outcomes).await;
        let results: Vec<crate::types::FileResult> = outcomes
            .iter()
            .filter_map(|outcome| outcome.as_result().cloned())
            .collect();
        let failures = outcomes
            .iter()
            .filter_map(|outcome| outcome.as_failure().cloned())
            .collect();
        let results = merge(
            results,
            &MergeOptions {
                provider_weights: self.provider_weights.clone(),
            },
        );
        SearchReport {
            results,
            failures,
            skipped,
        }
    }

    /// Like [`Engine::search`], but stream attributed [`Outcome`]s as they arrive
    /// instead of waiting for every provider.
    ///
    /// The returned [`SearchStream`] carries the providers skipped for this query
    /// and is itself a [`Stream`]; dropping it cancels in-flight providers.
    /// Outcomes are **not** merged or ranked (merging needs every result) — use
    /// this for interactive clients, and [`Engine::search`] for a ranked list.
    pub async fn search_stream(&self, query: FileQuery) -> SearchStream {
        let (providers, skipped) = self.capable(&query).await;
        SearchStream {
            skipped,
            outcomes: Box::pin(fanout(query, providers, self.options.clone())),
        }
    }

    /// Partition providers by whether their capabilities accept `query`,
    /// probing them concurrently so one slow provider doesn't stall the rest.
    async fn capable(&self, query: &FileQuery) -> (Vec<Arc<dyn SearchProvider>>, Vec<String>) {
        let checks = self.providers.iter().map(|provider| {
            let provider = provider.clone();
            let probe = query.clone();
            async move {
                let accepts = provider.capabilities().await.accepts(&probe);
                (accepts, provider)
            }
        });
        let evaluated = futures::future::join_all(checks).await;

        let mut providers = Vec::new();
        let mut skipped = Vec::new();
        for (accepts, provider) in evaluated {
            if accepts {
                providers.push(provider);
            } else {
                skipped.push(provider.name().to_owned());
            }
        }
        (providers, skipped)
    }

    /// Fetch the preferred address of a result.
    #[cfg(feature = "fetch")]
    pub async fn fetch(
        &self,
        result: &MergedResult,
        destination: impl Into<PathBuf>,
    ) -> Result<Fetched, FetchError> {
        self.fetchers.fetch_merged(result, destination).await
    }

    /// Like [`Engine::fetch`], but report cumulative retrieved bytes to
    /// `progress` as they arrive, so a client can render a progress bar.
    #[cfg(feature = "fetch")]
    pub async fn fetch_with_progress(
        &self,
        result: &MergedResult,
        destination: impl Into<PathBuf>,
        progress: &ProgressFn<'_>,
    ) -> Result<Fetched, FetchError> {
        self.fetchers
            .fetch_merged_with_progress(result, destination, progress)
            .await
    }

    /// Probe every provider with one query and classify its answer — for health
    /// checks and `providers` commands.
    ///
    /// Uses [`Options::timeout`] when set, otherwise a 10-second default. See
    /// [`crate::probe`] for the status semantics.
    pub async fn probe(&self, query: FileQuery) -> Vec<ProviderProbe> {
        let timeout = self
            .options
            .timeout
            .unwrap_or_else(|| Duration::from_secs(10));
        crate::search::probe(query, self.providers.clone(), timeout).await
    }
}

async fn collect(
    stream: impl futures::Stream<Item = crate::types::Outcome>,
) -> Vec<crate::types::Outcome> {
    futures::pin_mut!(stream);
    let mut outcomes = Vec::new();
    while let Some(outcome) = stream.next().await {
        outcomes.push(outcome);
    }
    outcomes
}
