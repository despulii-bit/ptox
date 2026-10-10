//! Drift protection: the JSON Schema, the spec vectors, and the Rust types must
//! describe the same contract.

use serde_json::{json, Value};

fn read(path: &str) -> Value {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|error| panic!("read {path}: {error}"));
    serde_json::from_str(&raw).unwrap_or_else(|error| panic!("parse {path}: {error}"))
}

fn schema() -> Value {
    read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../ptox/spec/schema/wire.schema.json"
    ))
}

fn vectors() -> Value {
    read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../ptox/spec/vectors.json"
    ))
}

fn validator(root: &Value, def: &str) -> jsonschema::JSONSchema {
    let defs = root.get("$defs").cloned().expect("schema has $defs");
    let subschema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$defs": defs,
        "$ref": format!("#/$defs/{def}")
    });
    jsonschema::JSONSchema::compile(&subschema).expect("compile schema")
}

fn assert_valid(root: &Value, def: &str, instance: &Value, label: &str) {
    let schema = validator(root, def);
    if !schema.is_valid(instance) {
        let errors: Vec<String> = schema
            .validate(instance)
            .err()
            .map(|errors| errors.map(|error| error.to_string()).collect())
            .unwrap_or_default();
        panic!("{label} does not match {def}: {}", errors.join("; "));
    }
}

#[test]
fn valid_spec_vectors_conform_to_the_schema() {
    let root = schema();
    let vectors = vectors();
    for (group, def) in [
        ("file_query", "FileQuery"),
        ("address", "Address"),
        ("file_result", "FileResult"),
        ("failure", "Failure"),
    ] {
        for case in vectors["valid"][group].as_array().expect("array") {
            let name = case["name"].as_str().unwrap_or("<unnamed>");
            assert_valid(&root, def, &case["value"], &format!("valid {group}/{name}"));
        }
    }
}

#[test]
fn rust_output_conforms_to_the_wire_schema() {
    use ptox::{encode_request, Address, ErrorInfo, Failure, FileQuery, FileResult};
    let root = schema();

    let request: Value = serde_json::from_str(
        encode_request(7, &FileQuery::new("x"))
            .expect("encode")
            .trim_end(),
    )
    .expect("json");
    assert_valid(&root, "Request", &request, "request");

    let mut result = FileResult::new("p", vec![Address::new("https", "https://example.org/x")]);
    result.title = Some("x".to_owned());
    assert_valid(
        &root,
        "ResultLine",
        &json!({ "id": 1, "result": result }),
        "result line",
    );

    let error = ErrorInfo {
        code: "malformed".into(),
        message: "bad".to_owned(),
    };
    assert_valid(
        &root,
        "ErrorLine",
        &json!({ "id": 1, "error": error }),
        "error line",
    );
    assert_valid(
        &root,
        "DoneLine",
        &json!({ "id": 1, "done": true }),
        "done line",
    );

    let failure = Failure {
        provider: "p".to_owned(),
        error: ErrorInfo {
            code: "timeout".into(),
            message: "slow".to_owned(),
        },
    };
    assert_valid(&root, "Failure", &json!(failure), "failure");
}
