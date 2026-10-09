//! The NDJSON wire format for out-of-process providers (contract §9).
//!
//! A provider is a child process that reads one request line from stdin and
//! writes result/error/done lines to stdout. This lets providers be written in
//! any language.

use crate::types::{ErrorInfo, FileQuery, FileResult};
use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt};

/// Wire protocol version, carried in every request as `v`.
/// See `spec/schema/wire.schema.json`.
pub const PROTOCOL_VERSION: u32 = 2;

/// Default maximum wire-line length. A longer line is treated as `malformed`,
/// so a hostile provider cannot exhaust the host's memory.
pub const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

/// Errors from reading a bounded NDJSON line.
#[derive(Debug, thiserror::Error)]
pub enum WireReadError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("wire line exceeded {limit} bytes")]
    TooLong { limit: usize },
}

/// Read one newline-terminated line, erroring if it exceeds `max` bytes.
///
/// Unlike `AsyncBufReadExt::lines`, this never grows an unbounded buffer.
pub async fn read_line_bounded<R>(
    reader: &mut R,
    max: usize,
) -> Result<Option<String>, WireReadError>
where
    R: AsyncBufRead + Unpin,
{
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let (finished, consumed) = {
            let available = reader.fill_buf().await?;
            if available.is_empty() {
                if buf.is_empty() {
                    return Ok(None);
                }
                (true, 0)
            } else if let Some(position) = available.iter().position(|byte| *byte == b'\n') {
                buf.extend_from_slice(&available[..position]);
                (true, position + 1)
            } else {
                buf.extend_from_slice(available);
                (false, available.len())
            }
        };

        if consumed > 0 {
            reader.consume(consumed);
        }
        if buf.len() > max {
            return Err(WireReadError::TooLong { limit: max });
        }
        if finished {
            break;
        }
    }

    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    String::from_utf8(buf).map(Some).map_err(|_| {
        WireReadError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid utf-8",
        ))
    })
}

/// One decoded line from a provider's stdout.
#[allow(clippy::large_enum_variant)] // a ~280-byte move beats a per-item allocation
#[derive(Debug, Clone, PartialEq)]
pub enum WireLine {
    Result(FileResult),
    Error(ErrorInfo),
    Done,
    Invalid { message: String },
}

#[derive(Serialize)]
struct Request<'a> {
    v: u32,
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
        v: PROTOCOL_VERSION,
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
        assert_eq!(value["v"], 2);
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

    #[tokio::test]
    async fn read_line_bounded_reads_until_newline_and_eof() {
        let mut reader = tokio::io::BufReader::new(&b"first\nsecond\n"[..]);
        assert_eq!(
            read_line_bounded(&mut reader, 64).await.expect("read"),
            Some("first".to_owned())
        );
        assert_eq!(
            read_line_bounded(&mut reader, 64).await.expect("read"),
            Some("second".to_owned())
        );
        assert_eq!(
            read_line_bounded(&mut reader, 64).await.expect("read"),
            None
        );
    }

    #[tokio::test]
    async fn read_line_bounded_rejects_over_limit() {
        let mut reader = tokio::io::BufReader::new(&b"aaaaaaaaaa\n"[..]);
        assert!(matches!(
            read_line_bounded(&mut reader, 4).await,
            Err(WireReadError::TooLong { limit: 4 })
        ));
    }
}
