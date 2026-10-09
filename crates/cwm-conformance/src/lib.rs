//! # cwm-conformance
//!
//! A contract conformance suite for cwm providers. It validates that a provider —
//! written in Rust (`check_in_process`) or as a subprocess speaking the NDJSON
//! wire format (`check_command`) — obeys `spec/core-contract.md`.
//!
//! Use it as a library or via the `cwm-conformance` binary:
//!
//! ```sh
//! cwm-conformance -- cwm-provider-example --config /etc/example.conf
//! ```

#![forbid(unsafe_code)]

mod command;
mod in_process;
mod report;

pub use command::check_command;
pub use in_process::check_in_process;
pub use report::{Check, Report};

use cwm_core::FileQuery;
use std::time::Duration;

/// What to run the provider against.
#[derive(Debug, Clone)]
pub struct Options {
    /// Queries sent to the provider. Each is an independent invocation.
    pub queries: Vec<FileQuery>,
    /// Time budget per query; exceeding it fails the termination check.
    pub timeout: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            queries: vec![FileQuery::new("conformance test")],
            timeout: Duration::from_secs(10),
        }
    }
}
