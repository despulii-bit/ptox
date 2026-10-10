//! # ptox
//!
//! The **ptox SDK** — a language-neutral toolkit for building file-search clients
//! that work across many networks. It has three parts:
//!
//! - **Contracts** — [`types`] (`FileQuery`, `FileResult`, `Address`, …), the
//!   [`search::SearchProvider`] trait, and the [`fetch::Fetcher`] trait.
//! - **Engines** — [`search::fanout`], the [`fetch::FetchRegistry`], and the
//!   [`merge`] post-processor.
//! - **Tooling** — the NDJSON [`provider`] bridge (the conformance suite is the
//!   separate `ptox-conformance` crate).
//!
//! Providers (network plugins) and clients (frontends) are built on top.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod engine;
pub mod merge;
pub mod provider;
pub mod search;
pub mod types;

#[cfg(feature = "fetch")]
pub mod fetch;

pub use engine::{Engine, SearchReport, SearchStream};
pub use merge::{merge, suggest_filename, MergeOptions, MergedResult};
pub use provider::*;
pub use search::*;
pub use types::*;

#[cfg(feature = "fetch")]
pub use fetch::*;
