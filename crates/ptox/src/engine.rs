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
use crate::search::{
    fanout, fanout_with_metrics, Capabilities, Metrics, Options, ProviderCapabilities,
    ProviderMetric, ProviderProbe, ResultStream, SearchProvider,
};
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
    /// Per-provider counts and timings, sorted by provider name. Carries no
    /// query or result content; see [`ProviderMetric`].
    pub metrics: Vec<ProviderMetric>,
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
    /// Memoized declared capabilities, installed by [`Engine::with_capability_cache`].
    capabilities: Option<Arc<CapabilityCache>>,
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
            capabilities: None,
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
            capabilities: None,
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

    /// The providers this engine was built with. When
    /// [`Engine::with_capability_cache`] is enabled these are transparent
    /// wrappers that delegate `name`/`search` and memoize `capabilities`.
    pub fn providers(&self) -> &[Arc<dyn SearchProvider>] {
        &self.providers
    }

    /// Memoize each provider's declared [`Capabilities`] after the first probe.
    ///
    /// Capabilities are requested before every [`Engine::search`],
    /// [`Engine::search_stream`], and [`Engine::probe`] call, and a provider is
    /// free to compute them dynamically. With this opt-in cache the engine asks
    /// each provider once and reuses the answer, so an in-process provider with
    /// an expensive capability query pays it only once. This is a pure
    /// optimization: it never changes which queries a provider receives, only
    /// how often its declarations are recomputed. Call
    /// [`Engine::invalidate_capabilities`] to drop the cache and force a
    /// re-probe.
    ///
    /// Off by default; leave it off for a provider whose capabilities can
    /// change between searches, or invalidate explicitly. Providers that cache
    /// internally (such as [`crate::SubprocessProvider`]) are unaffected by
    /// this flag and always pay their own probe at most once.
    pub fn with_capability_cache(mut self) -> Self {
        if self.capabilities.is_some() {
            return self;
        }
        let cache = Arc::new(CapabilityCache::default());
        self.capabilities = Some(cache.clone());
        self.providers = std::mem::take(&mut self.providers)
            .into_iter()
            .map(|inner| {
                let key = Arc::as_ptr(&inner) as *const () as usize;
                Arc::new(CapabilityCachedProvider {
                    inner,
                    key,
                    cache: cache.clone(),
                }) as Arc<dyn SearchProvider>
            })
            .collect();
        self
    }

    /// Drop every memoized [`Capabilities`] answer (see
    /// [`Engine::with_capability_cache`]) and ask each provider to drop its own
    /// internal capability cache, so the next search or probe re-probes.
    ///
    /// The engine-level cache is a no-op when it was never enabled, but
    /// providers are always asked to invalidate their own caches.
    pub fn invalidate_capabilities(&self) {
        if let Some(cache) = &self.capabilities {
            cache.clear();
        }
        for provider in &self.providers {
            provider.invalidate_capabilities();
        }
    }

    /// Run the query across every capable provider; return ranked, deduplicated
    /// results. Providers whose declared capabilities reject the query are
    /// skipped (listed in [`SearchReport::skipped`]) without being spawned.
    ///
    /// The report also carries per-provider [`ProviderMetric`]s (result/failure
    /// counts and elapsed time), so a client can show progress and diagnostics.
    pub async fn search(&self, query: FileQuery) -> SearchReport {
        let (providers, skipped) = self.capable(&query).await;
        let metrics = Metrics::new();
        let outcomes = collect(fanout_with_metrics(
            query,
            providers,
            self.options.clone(),
            &metrics,
        ))
        .await;
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
            metrics: metrics.snapshot(),
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

    /// Fetch a result, trying its addresses in preference order until one
    /// succeeds (multi-source failover), and verifying its sha256.
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
    ///
    /// Progress stays monotonic across failover; see
    /// [`FetchRegistry::fetch_merged_with_progress`].
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

    /// Ask every provider for its declared [`Capabilities`], without running a
    /// query — for a `providers` or diagnostics command.
    ///
    /// Probes run concurrently and reuse the [`Engine::with_capability_cache`]
    /// memo when enabled. Uses [`Options::timeout`] when set, otherwise a
    /// 10-second default. A provider that does not answer before the timeout is
    /// reported with [`Capabilities::universal`] and `answered: false`; this
    /// call never fails. See [`crate::search::provider_capabilities`].
    pub async fn provider_capabilities(&self) -> Vec<ProviderCapabilities> {
        let timeout = self
            .options
            .timeout
            .unwrap_or_else(|| Duration::from_secs(10));
        crate::search::provider_capabilities(self.providers.clone(), timeout).await
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

/// A cache of each provider's declared [`Capabilities`], keyed by provider
/// identity. Installed by [`Engine::with_capability_cache`].
#[derive(Default)]
struct CapabilityCache {
    entries: std::sync::Mutex<HashMap<usize, Capabilities>>,
}

impl CapabilityCache {
    fn get(&self, key: usize) -> Option<Capabilities> {
        self.entries
            .lock()
            .ok()
            .and_then(|entries| entries.get(&key).cloned())
    }

    fn insert(&self, key: usize, capabilities: Capabilities) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(key, capabilities);
        }
    }

    fn remove(&self, key: usize) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(&key);
        }
    }

    fn clear(&self) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.clear();
        }
    }
}

/// A provider wrapper that memoizes [`SearchProvider::capabilities`].
///
/// Installed by [`Engine::with_capability_cache`] on every provider, so both
/// [`Engine::search`] and [`Engine::probe`] pay the probe cost at most once per
/// provider until [`Engine::invalidate_capabilities`] is called.
struct CapabilityCachedProvider {
    inner: Arc<dyn SearchProvider>,
    key: usize,
    cache: Arc<CapabilityCache>,
}

#[async_trait::async_trait]
impl SearchProvider for CapabilityCachedProvider {
    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn capabilities(&self) -> Capabilities {
        if let Some(capabilities) = self.cache.get(self.key) {
            return capabilities;
        }
        let capabilities = self.inner.capabilities().await;
        self.cache.insert(self.key, capabilities.clone());
        capabilities
    }

    fn invalidate_capabilities(&self) {
        self.cache.remove(self.key);
        self.inner.invalidate_capabilities();
    }

    fn search(&self, query: &FileQuery) -> ResultStream<'_> {
        self.inner.search(query)
    }
}
