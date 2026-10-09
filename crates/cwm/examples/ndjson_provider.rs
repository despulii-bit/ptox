//! A tiny example provider speaking the NDJSON wire format.
//!
//! Usage: `ndjson_provider [ok|error|malformed|infinite]`
//! It reads one request line from stdin and writes NDJSON response lines to
//! stdout. Used by the subprocess-provider integration tests.

use std::io::{self, BufRead, Write};

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "ok".to_owned());

    let mut line = String::new();
    let _ = io::stdin().lock().read_line(&mut line);

    let stdout = io::stdout();
    let mut out = stdout.lock();

    match mode.as_str() {
        "malformed" => {
            let _ = writeln!(out, "this is not json");
        }
        "error" => {
            let _ = writeln!(
                out,
                r#"{{"id":1,"error":{{"code":"refused","message":"nope"}}}}"#
            );
        }
        "infinite" => {
            let mut i: u64 = 0;
            loop {
                let line = format!(
                    r#"{{"id":1,"result":{{"provider":"example","title":"f{i}.bin","addresses":[{{"scheme":"https","uri":"https://example.org/f{i}.bin"}}]}}}}"#
                );
                if writeln!(out, "{line}").is_err() {
                    return;
                }
                i += 1;
            }
        }
        "crash" => {
            std::process::exit(3);
        }
        "capabilities" => {
            let _ = writeln!(
                out,
                r#"{{"id":1,"result":{{"query":["text"],"schemes":["https"]}}}}"#
            );
            let _ = writeln!(out, r#"{{"id":1,"done":true}}"#);
        }
        _ => {
            let text = serde_json::from_str::<serde_json::Value>(&line)
                .ok()
                .and_then(|value| value["params"]["query"]["text"].as_str().map(str::to_owned))
                .unwrap_or_default();
            let tag = std::env::var("CWM_EXAMPLE_TAG").unwrap_or_default();
            let prefix = if tag.is_empty() {
                String::new()
            } else {
                format!("{tag}-")
            };
            let first = format!(
                r#"{{"id":1,"result":{{"provider":"example","title":"{prefix}{text}-0.bin","addresses":[{{"scheme":"https","uri":"https://example.org/{prefix}{text}-0.bin"}}]}}}}"#
            );
            let second = format!(
                r#"{{"id":1,"result":{{"provider":"example","title":"{prefix}{text}-1.bin","addresses":[{{"scheme":"https","uri":"https://example.org/{prefix}{text}-1.bin"}}]}}}}"#
            );
            let _ = writeln!(out, "{first}");
            let _ = writeln!(out, "{second}");
            let _ = writeln!(out, r#"{{"id":1,"done":true}}"#);
        }
    }

    let _ = out.flush();
}
