use cwm_core::{Address, Failure, FileQuery, FileResult, Validate};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use std::fmt::Debug;

fn load() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../spec/vectors.json");
    let raw = std::fs::read_to_string(path).expect("read spec/vectors.json");
    serde_json::from_str(&raw).expect("parse spec/vectors.json")
}

fn check_valid<T>(vectors: &Value, type_name: &str)
where
    T: DeserializeOwned + Serialize + Validate + PartialEq + Debug,
{
    for case in vectors["valid"][type_name]
        .as_array()
        .unwrap_or_else(|| panic!("valid.{type_name} must be an array"))
    {
        let name = case["name"].as_str().unwrap_or("<unnamed>");
        let typed: T = serde_json::from_value(case["value"].clone()).unwrap_or_else(|error| {
            panic!("valid {type_name}/{name}: deserialize failed: {error}")
        });
        typed
            .validate()
            .unwrap_or_else(|error| panic!("valid {type_name}/{name}: validate failed: {error}"));

        // Known fields must round-trip without loss.
        let json = serde_json::to_value(&typed).expect("serialize");
        let back: T = serde_json::from_value(json).expect("re-deserialize");
        assert_eq!(typed, back, "round-trip mismatch for {type_name}/{name}");
    }
}

fn check_invalid<T>(vectors: &Value, type_name: &str)
where
    T: DeserializeOwned + Validate + Debug,
{
    for case in vectors["invalid"][type_name]
        .as_array()
        .unwrap_or_else(|| panic!("invalid.{type_name} must be an array"))
    {
        let name = case["name"].as_str().unwrap_or("<unnamed>");
        let accepted = match serde_json::from_value::<T>(case["value"].clone()) {
            Err(_) => false,
            Ok(typed) => typed.validate().is_ok(),
        };
        assert!(!accepted, "invalid {type_name}/{name} was accepted");
    }
}

#[test]
fn valid_vectors_pass() {
    let vectors = load();
    check_valid::<FileQuery>(&vectors, "file_query");
    check_valid::<Address>(&vectors, "address");
    check_valid::<FileResult>(&vectors, "file_result");
    check_valid::<Failure>(&vectors, "failure");
}

#[test]
fn invalid_vectors_are_rejected() {
    let vectors = load();
    check_invalid::<FileQuery>(&vectors, "file_query");
    check_invalid::<Address>(&vectors, "address");
    check_invalid::<FileResult>(&vectors, "file_result");
    check_invalid::<Failure>(&vectors, "failure");
}
