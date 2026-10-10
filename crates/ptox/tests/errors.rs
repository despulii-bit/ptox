//! `Display` stability for the public error types (regression guard).
//!
//! Error strings are client-facing: they land in logs and terminals, and docs
//! quote them. These tests pin every well-known [`ptox::ErrorCode`] constructor
//! and every [`ptox::FetchError`] / [`ptox::ValidationError`] variant to an
//! exact, non-empty message, so a change cannot silently blank one or start
//! interpolating extra (possibly sensitive) detail.

use ptox::{ErrorCode, ValidationError};

#[test]
fn error_code_constructors_have_stable_display() {
    assert_eq!(ErrorCode::timeout().to_string(), "timeout");
    assert_eq!(ErrorCode::unsupported().to_string(), "unsupported");
    assert_eq!(ErrorCode::refused().to_string(), "refused");
    assert_eq!(ErrorCode::transport().to_string(), "transport");
    assert_eq!(ErrorCode::unavailable().to_string(), "unavailable");
    assert_eq!(ErrorCode::malformed().to_string(), "malformed");
    assert_eq!(ErrorCode::cancelled().to_string(), "cancelled");
    assert_eq!(ErrorCode::internal().to_string(), "internal");
}

#[test]
fn error_code_custom_values_display_verbatim() {
    assert_eq!(ErrorCode::new("vendor.thing").to_string(), "vendor.thing");
    assert_eq!(ErrorCode::from("from-str").to_string(), "from-str");
    assert_eq!(
        ErrorCode::from(String::from("from-string")).to_string(),
        "from-string"
    );
}

#[test]
fn validation_error_variants_have_stable_display() {
    assert_eq!(
        ValidationError::Empty("provider").to_string(),
        "field `provider` must not be empty"
    );
    assert_eq!(
        ValidationError::NoAddresses.to_string(),
        "`addresses` must not be empty"
    );
    assert_eq!(
        ValidationError::SizeRange { min: 10, max: 5 }.to_string(),
        "size_min (10) must not exceed size_max (5)"
    );
}

#[cfg(feature = "fetch")]
#[test]
fn fetch_error_variants_have_stable_display() {
    use std::io;

    assert_eq!(
        ptox::FetchError::Unsupported("gopher".to_owned()).to_string(),
        "unsupported address scheme `gopher`"
    );
    assert_eq!(
        ptox::FetchError::Io(io::Error::new(io::ErrorKind::NotFound, "missing")).to_string(),
        "io error: missing"
    );
    assert_eq!(
        ptox::FetchError::Network("connection reset".to_owned()).to_string(),
        "network error: connection reset"
    );
    assert_eq!(
        ptox::FetchError::Command("helper exited 3".to_owned()).to_string(),
        "command failed: helper exited 3"
    );
    assert_eq!(
        ptox::FetchError::HashMismatch {
            expected: "aa".to_owned(),
            actual: "bb".to_owned(),
        }
        .to_string(),
        "hash mismatch: expected aa, got bb"
    );
    assert_eq!(
        ptox::FetchError::NoAddress.to_string(),
        "result has no address"
    );
    assert_eq!(
        ptox::FetchError::InvalidAddress("missing CID".to_owned()).to_string(),
        "invalid address: missing CID"
    );
    assert_eq!(
        ptox::FetchError::AllFailed {
            count: 3,
            errors: vec![ptox::FetchError::NoAddress],
        }
        .to_string(),
        "all 3 address(es) failed"
    );
    assert_eq!(
        ptox::FetchError::AccessDenied("/etc/shadow".to_owned()).to_string(),
        "access denied: /etc/shadow"
    );
}
