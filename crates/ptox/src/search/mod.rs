//! The **search half** of ptox: the [`SearchProvider`] contract and the [`fanout`]
//! engine that runs providers concurrently and streams attributed results.

pub mod capabilities;
mod fanout;
mod probe;
mod provider;

pub use capabilities::Capabilities;
pub use fanout::{fanout, fanout_with_metrics, Metrics, Options, ProviderMetric};
pub use probe::{probe, provider_capabilities, ProbeStatus, ProviderCapabilities, ProviderProbe};
pub use provider::{ProviderError, ResultStream, SearchProvider};
