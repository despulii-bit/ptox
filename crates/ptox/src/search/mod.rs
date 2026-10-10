//! The **search half** of ptox: the [`SearchProvider`] contract and the [`fanout`]
//! engine that runs providers concurrently and streams attributed results.

pub mod capabilities;
mod fanout;
mod probe;
mod provider;

pub use capabilities::Capabilities;
pub use fanout::{fanout, Options};
pub use probe::{probe, ProbeStatus, ProviderProbe};
pub use provider::{ProviderError, ResultStream, SearchProvider};
