//! # cwm
//!
//! The **cwm SDK** — the single dependency a client builds on. Re-exports:
//!
//! - [`cwm_types`] — the data contracts.
//! - [`cwm_search`] — the search engine and `SearchProvider` contract.
//! - [`cwm_provider`] — the NDJSON provider bridge and `SubprocessProvider`.
//! - [`cwm_fetch`] — the retrieval fabric and `Fetcher` contract.
//!
//! Providers (network plugins) and clients (frontends) are built on top and live
//! in their own repos.
#![forbid(unsafe_code)]

pub use cwm_fetch::*;
pub use cwm_provider::*;
pub use cwm_search::*;
pub use cwm_types::*;
