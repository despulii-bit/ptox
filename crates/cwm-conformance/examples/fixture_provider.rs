//! Test fixture provider used by the conformance suite's own tests.
//!
//! Usage: `fixture_provider [good|empty|invalid-json|bad-shape|hang]`
//! Reads one request line and writes NDJSON responses to stdout.

use std::io::{self, BufRead, Write};

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "good".to_owned());

    let mut line = String::new();
    let _ = io::stdin().lock().read_line(&mut line);

    let stdout = io::stdout();
    let mut out = stdout.lock();

    match mode.as_str() {
        "empty" => {
            let _ = writeln!(out, r#"{{"id":1,"done":true}}"#);
        }
        "invalid-json" => {
            let _ = writeln!(out, "this is not json");
        }
        "bad-shape" => {
            // Empty provider + no addresses: must fail shape validation.
            let _ = writeln!(
                out,
                r#"{{"id":1,"result":{{"provider":"","title":"x","addresses":[]}}}}"#
            );
            let _ = writeln!(out, r#"{{"id":1,"done":true}}"#);
        }
        "hang" => {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
        "oversized" => {
            // A line far larger than the host's limit.
            let big = "a".repeat(4096);
            let _ = writeln!(out, "{big}");
        }
        _ => {
            // Note: no `provider` field; the host is responsible for attribution.
            let _ = writeln!(
                out,
                r#"{{"id":1,"result":{{"title":"x","addresses":[{{"scheme":"https","uri":"https://example.org/x"}}]}}}}"#
            );
            let _ = writeln!(out, r#"{{"id":1,"done":true}}"#);
        }
    }

    let _ = out.flush();
}
