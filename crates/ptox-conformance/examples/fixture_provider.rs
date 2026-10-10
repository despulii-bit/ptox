//! Test fixture provider used by the conformance suite's own tests.
//!
//! Usage:
//! `fixture_provider [good|empty|invalid-json|bad-shape|hang|oversized|crash|capabilities|bad-capabilities|scheme-mismatch|limit-ignored]`
//!
//! Reads one request line and writes NDJSON responses to stdout.
//!
//! `capabilities`, `bad-capabilities`, and `scheme-mismatch` only change the
//! answer to a `capabilities` request; every other mode declines capability
//! negotiation (answering `done`) and behaves as before for `search`.
//! `scheme-mismatch` declares only `magnet` while its `search` emits `https`.
//! `limit-ignored` emits more results than the suite's `limit` probe allows.

use std::io::{self, BufRead, Write};

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "good".to_owned());

    let mut line = String::new();
    let _ = io::stdin().lock().read_line(&mut line);

    let method = serde_json::from_str::<serde_json::Value>(&line)
        .ok()
        .and_then(|value| {
            value
                .get("method")
                .and_then(|method| method.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_default();

    let stdout = io::stdout();
    let mut out = stdout.lock();

    if method == "capabilities" {
        match mode.as_str() {
            "capabilities" => {
                let _ = writeln!(
                    out,
                    r#"{{"id":1,"result":{{"query":["text","hashes"],"requires":[],"schemes":["https","magnet"]}}}}"#
                );
            }
            "scheme-mismatch" => {
                // Declares only `magnet`, but search below emits `https`.
                let _ = writeln!(out, r#"{{"id":1,"result":{{"schemes":["magnet"]}}}}"#);
            }
            "bad-capabilities" => {
                // `query` must be a list of strings; a bare string is malformed.
                let _ = writeln!(out, r#"{{"id":1,"result":{{"query":"not-a-list"}}}}"#);
            }
            _ => {
                let _ = writeln!(out, r#"{{"id":1,"done":true}}"#);
            }
        }
        let _ = out.flush();
        return;
    }

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
        "crash" => {
            // Exit non-zero with no output.
            std::process::exit(3);
        }
        "oversized" => {
            // A line far larger than the host's limit.
            let big = "a".repeat(4096);
            let _ = writeln!(out, "{big}");
        }
        "limit-ignored" => {
            // Emits more results than the suite's limit probe allows.
            for index in 0..3 {
                let _ = writeln!(
                    out,
                    r#"{{"id":1,"result":{{"title":"x{index}","addresses":[{{"scheme":"https","uri":"https://example.org/x{index}"}}]}}}}"#
                );
            }
            let _ = writeln!(out, r#"{{"id":1,"done":true}}"#);
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
