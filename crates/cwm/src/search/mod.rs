//! The **search half** of cwm: the [`SearchProvider`] contract and the [`fanout`]
//! engine that runs providers concurrently and streams attributed results.

pub mod capabilities;
mod fanout;
mod provider;

pub use capabilities::Capabilities;
pub use fanout::{fanout, Options};
pub use provider::{ProviderError, ResultStream, SearchProvider};
