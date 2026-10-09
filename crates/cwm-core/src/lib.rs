//! # cwm-core
//!
//! The cwm core does exactly one thing: take a [`FileQuery`], fan it out to a set
//! of injected [`SearchProvider`]s, and stream back attributed [`Outcome`]s.
//!
//! The core has **no state, no policy, no discovery, and no network I/O**. It
//! never fetches anything referenced by an [`Address`]; retrieval, ranking,
//! storage, discovery, and frontends are separate software built on this
//! contract. See `spec/core-contract.md`.

#![forbid(unsafe_code)]

mod fanout;
mod provider;
mod types;

pub use fanout::{fanout, Options};
pub use provider::{ProviderError, ResultStream, SearchProvider};
pub use types::{
    Address, ErrorCode, ErrorInfo, Extras, Failure, FileQuery, FileResult, HashAlgos, Outcome,
    Validate, ValidationError,
};
