//! # ptox-conformance
//!
//! A contract conformance suite for ptox providers. It validates that a provider —
//! written in Rust (`check_in_process`) or as a subprocess speaking the NDJSON
//! wire format (`check_command`) — obeys the core contract
//! (`../ptox/spec/core-contract.md`).
//!
//! Use it as a library or via the `ptox-conformance` binary:
//!
//! ```sh
//! ptox-conformance -- ptox-provider-example --config /etc/example.conf
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod command;
mod in_process;
mod report;
mod schemes;

pub use command::check_command;
pub use in_process::check_in_process;
pub use report::{Check, Report};

use ptox::FileQuery;
use std::time::Duration;

/// What to run the provider against.
#[derive(Debug, Clone)]
pub struct Options {
    /// Queries sent to the provider. Each is an independent invocation.
    pub queries: Vec<FileQuery>,
    /// Time budget per query; exceeding it fails the termination check.
    pub timeout: Duration,
    /// Maximum accepted wire-line length (hostile-output bound).
    pub max_line_bytes: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            queries: vec![FileQuery::new("conformance test")],
            timeout: Duration::from_secs(10),
            max_line_bytes: ptox::MAX_LINE_BYTES,
        }
    }
}

/// The name of the check that verifies a provider honors `limit`.
pub(crate) const LIMIT_CHECK: &str = "limit_honored";

/// The `limit` carried by the suite's probe query.
pub(crate) const LIMIT_PROBE: u32 = 1;

/// The probe query used to verify that a provider honors `limit`.
///
/// The suite sends it once, separately from [`Options::queries`], and requires
/// the provider to return at most [`LIMIT_PROBE`] results. A provider that
/// returns fewer still passes; one that ignores the selector is caught.
pub(crate) fn limit_probe() -> FileQuery {
    FileQuery::new("conformance limit probe").with_limit(LIMIT_PROBE)
}
