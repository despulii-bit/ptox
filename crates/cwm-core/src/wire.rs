//! The NDJSON wire format for out-of-process providers (contract §9).
//!
//! A provider is a child process that reads one request line from stdin and
//! writes result/error/done lines to stdout. This lets providers be written in
//! any language.

use crate::types::{ErrorInfo, FileQuery, FileResult};
use serde::Serialize;
use serde_json::Value;

/// One decoded line from a provider's stdout.
#[derive(Debug, Clone, PartialEq)]
pub enum WireLine {
    Result(FileResult),
    Error(ErrorInfo),
    Done,
    Invalid { message: String },
}

#[derive(Serialize)]
struct Request<'a> {
    id: u64,
    method: &'static str,
    params: Params<'a>,
}

#[derive(Serialize)]
struct Params<'a> {
    query: &'a FileQuery,
}

/// Encode a `search` request as a single NDJSON line.
pub fn encode_request(id: u64, query: &FileQuery) -> Result<String, serde_json::Error> {
    let request = Request {
        id,
        method: "search",
        params: Params { query },
    };
    Ok(serde_json::to_string(&request)? + "\n")
}

/// Decode one NDJSON line from a provider. Unknown fields are tolerated.
pub fn parse_line(line: &str) -> WireLine {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            return WireLine::Invalid {
                message: format!("invalid json: {error}"),
            }
        }
    };

    if value.get("done").and_then(Value::as_bool) == Some(true) {
        return WireLine::Done;
    }

    if let Some(error) = value.get("error") {
        return match serde_json::from_value::<ErrorInfo>(error.clone()) {
            Ok(error) => WireLine::Error(error),
            Err(error) => WireLine::Invalid {
                message: format!("invalid error object: {error}"),
            },
        };
    }

    if let Some(result) = value.get("result") {
        return match serde_json::from_value::<FileResult>(result.clone()) {
            Ok(result) => WireLine::Result(result),
            Err(error) => WireLine::Invalid {
                message: format!("invalid result object: {error}"),
            },
        };
    }

    WireLine::Invalid {
        message: "line has neither `result`, `error`, nor `done`".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ErrorCode;

    #[test]
    fn encodes_a_search_request_as_ndjson() {
        let line = encode_request(7, &FileQuery::new("hello")).expect("encode");
        assert!(line.ends_with('\n'));
        let value: Value = serde_json::from_str(line.trim_end()).expect("json");
        assert_eq!(value["id"], 7);
        assert_eq!(value["method"], "search");
        assert_eq!(value["params"]["query"]["text"], "hello");
    }

    #[test]
    fn parses_result_error_done_and_invalid() {
        let result = r#"{"id":1,"result":{"provider":"p","title":"t","addresses":[{"scheme":"https","uri":"https://e/x"}]}}"#;
        match parse_line(result) {
            WireLine::Result(result) => {
                assert_eq!(result.provider, "p");
                assert_eq!(result.addresses.len(), 1);
            }
            other => panic!("expected result, got {other:?}"),
        }

        let error = r#"{"id":1,"error":{"code":"timeout","message":"slow"}}"#;
        match parse_line(error) {
            WireLine::Error(error) => assert_eq!(error.code, ErrorCode::timeout()),
            other => panic!("expected error, got {other:?}"),
        }

        assert_eq!(parse_line(r#"{"id":1,"done":true}"#), WireLine::Done);
        assert!(matches!(parse_line("not json"), WireLine::Invalid { .. }));
        assert!(matches!(
            parse_line(r#"{"id":1}"#),
            WireLine::Invalid { .. }
        ));
    }
}
