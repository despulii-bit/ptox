//! The high-level entry point: providers in, ranked results and fetched files out.
//!
//! [`Engine`] bundles the search pipeline ([`crate::fanout`] + [`crate::merge`]) and,
//! with the `fetch` feature, retrieval. A client is then a few lines:
//!
//! ```no_run
//! # async fn run(providers: Vec<std::sync::Arc<dyn cwm::SearchProvider>>) {
//! let engine = cwm::Engine::new(providers);
//! let report = engine.search(cwm::FileQuery::new("annual report")).await;
//! for result in &report.results {
//!     println!("{:?} from {:?}", result.title, result.providers);
//! }
//! # }
//! ```

use crate::merge::{merge, MergeOptions, MergedResult};
use crate::search::{fanout, Options, SearchProvider};
use crate::types::{Failure, FileQuery};
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;

#[cfg(feature = "fetch")]
use crate::fetch::{FetchError, FetchRegistry, Fetched};
#[cfg(feature = "fetch")]
use std::path::PathBuf;

/// The outcome of one search: ranked results plus per-provider failures.
pub struct SearchReport {
    pub results: Vec<MergedResult>,
    pub failures: Vec<Failure>,
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
    #[cfg(feature = "fetch")]
    pub fn new(providers: Vec<Arc<dyn SearchProvider>>) -> Self {
        Self {
            providers,
            options: Options::default(),
            provider_weights: HashMap::new(),
            fetchers: FetchRegistry::with_defaults(),
        }
    }

    #[cfg(not(feature = "fetch"))]
    pub fn new(providers: Vec<Arc<dyn SearchProvider>>) -> Self {
        Self {
            providers,
            options: Options::default(),
            provider_weights: HashMap::new(),
        }
    }

    pub fn with_options(mut self, options: Options) -> Self {
        self.options = options;
        self
    }

    pub fn with_weights(mut self, provider_weights: HashMap<String, f64>) -> Self {
        self.provider_weights = provider_weights;
        self
    }

    #[cfg(feature = "fetch")]
    pub fn with_fetchers(mut self, fetchers: FetchRegistry) -> Self {
        self.fetchers = fetchers;
        self
    }

    pub fn providers(&self) -> &[Arc<dyn SearchProvider>] {
        &self.providers
    }

    /// Run the query across every provider and return ranked, deduplicated results.
    pub async fn search(&self, query: FileQuery) -> SearchReport {
        let outcomes = collect(fanout(query, self.providers.clone(), self.options.clone())).await;
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
        SearchReport { results, failures }
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
