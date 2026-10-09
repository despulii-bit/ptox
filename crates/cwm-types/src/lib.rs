//! # cwm-types
//!
//! The cwm **data contracts**: [`FileQuery`], [`FileResult`], [`Address`],
//! [`Failure`], [`Outcome`], and [`Validate`]. Pure data — no engine, no I/O.
#![forbid(unsafe_code)]

mod types;

pub use types::{
    Address, ErrorCode, ErrorInfo, Extras, Failure, FileQuery, FileResult, HashAlgos, Outcome,
    Validate, ValidationError,
};
