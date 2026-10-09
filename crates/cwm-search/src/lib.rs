//! # cwm-search
//!
//! The **search half** of cwm: the [`SearchProvider`] contract and the [`fanout`]
//! engine that runs providers concurrently and streams attributed results.
#![forbid(unsafe_code)]

mod fanout;
mod provider;

pub use fanout::{fanout, Options};
pub use provider::{ProviderError, ResultStream, SearchProvider};
