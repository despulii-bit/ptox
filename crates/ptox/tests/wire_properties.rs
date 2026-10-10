//! Deterministic property tests for the NDJSON wire boundary.
//!
//! A provider is an untrusted child process: everything it writes on stdout
//! reaches [`parse_line`] and [`read_line_bounded`]. These tests drive both with
//! pseudo-random bytes and values and assert the decoder (a) never panics, (b)
//! never returns a line longer than its configured bound, and (c) round-trips
//! valid NDJSON, including multi-byte UTF-8 split across arbitrary buffer
//! boundaries. There is deliberately no fuzzing dependency: the PRNG is a fixed
//! splitmix64, so any failure is reproducible from the seed printed in the
//! assertion message.

use ptox::{
    encode_capabilities_request, encode_request, parse_line, read_line_bounded, Address, ErrorCode,
    ErrorInfo, FileQuery, FileResult, HashAlgos, WireLine, WireReadError, MAX_LINE_BYTES,
};
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncBufRead, AsyncRead, ReadBuf};

/// A tiny deterministic PRNG (splitmix64). `below(0)` is defined as 0.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, upper: usize) -> usize {
        if upper == 0 {
            0
        } else {
            (self.next_u64() % upper as u64) as usize
        }
    }

    fn byte(&mut self) -> u8 {
        (self.next_u64() >> 56) as u8
    }

    fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.byte()).collect()
    }

    /// An arbitrary Unicode scalar value, including astral and control chars.
    fn char(&mut self) -> char {
        loop {
            if let Some(c) = char::from_u32((self.next_u64() % 0x11_0000) as u32) {
                return c;
            }
        }
    }

    fn unicode(&mut self, max_len: usize) -> String {
        (0..self.below(max_len + 1)).map(|_| self.char()).collect()
    }

    fn ascii(&mut self, max_len: usize) -> String {
        (0..self.below(max_len + 1))
            .map(|_| char::from_u32(0x20 + self.below(95) as u32).expect("printable ascii"))
            .collect()
    }

    fn word(&mut self, max_len: usize) -> String {
        (0..1 + self.below(max_len))
            .map(|_| char::from_u32(0x61 + self.below(26) as u32).expect("lowercase ascii"))
            .collect()
    }
}

fn random_size(rng: &mut Rng) -> Option<u64> {
    if rng.below(3) == 0 {
        Some(rng.next_u64())
    } else {
        None
    }
}

fn random_hashes(rng: &mut Rng) -> Option<HashAlgos> {
    if rng.below(2) == 0 {
        return None;
    }
    let mut hashes = HashAlgos::new();
    for _ in 0..1 + rng.below(3) {
        hashes.insert(rng.word(8), format!("{:x}", rng.next_u64()));
    }
    Some(hashes)
}

fn random_address(rng: &mut Rng) -> Address {
    Address {
        scheme: rng.word(6),
        uri: format!("{}://{}", rng.word(4), rng.ascii(12)),
        hashes: random_hashes(rng),
        size: random_size(rng),
        extras: None,
    }
}

fn random_file_result(rng: &mut Rng) -> FileResult {
    let addresses = (0..1 + rng.below(3)).map(|_| random_address(rng)).collect();
    let mut result = FileResult::new(rng.word(6), addresses);
    if rng.below(2) == 0 {
        result.title = Some(rng.unicode(24));
    }
    result.size = random_size(rng);
    result.hashes = random_hashes(rng);
    if rng.below(2) == 0 {
        result.mimetype = Some(rng.ascii(16));
    }
    if rng.below(2) == 0 {
        result.lang = Some(rng.word(5));
    }
    if rng.below(2) == 0 {
        result.published = Some(rng.ascii(20));
    }
    if rng.below(2) == 0 {
        result.last_seen = Some(rng.ascii(20));
    }
    if rng.below(2) == 0 {
        // Finite, so serialization always succeeds and round-trips exactly.
        result.score = Some(rng.below(1000) as f64 / 8.0);
    }
    if rng.below(2) == 0 {
        result.raw = Some(serde_json::json!({ "note": rng.ascii(8) }));
    }
    result
}

fn random_error(rng: &mut Rng) -> ErrorInfo {
    ErrorInfo {
        code: ErrorCode(rng.word(8)),
        message: rng.ascii(24),
    }
}

/// One line of arbitrary content. Newlines are excluded so the line is a single
/// wire record; a trailing `\r` is excluded so it is not stripped as a CRLF.
fn random_line(rng: &mut Rng) -> String {
    (0..rng.below(24))
        .map(|_| rng.char())
        .filter(|c| *c != '\n' && *c != '\r')
        .collect()
}

/// An `AsyncBufRead` that hands out at most `chunk` bytes per `fill_buf`, so a
/// multi-byte UTF-8 character can be split across buffer boundaries.
struct Chunked {
    data: Vec<u8>,
    position: usize,
    chunk: usize,
}

impl AsyncRead for Chunked {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let remaining = &self.data[self.position..];
        let take = remaining.len().min(self.chunk).min(buf.remaining());
        buf.put_slice(&remaining[..take]);
        self.position += take;
        Poll::Ready(Ok(()))
    }
}

impl AsyncBufRead for Chunked {
    fn poll_fill_buf(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<&[u8]>> {
        let this = self.get_mut();
        let take = (this.data.len() - this.position).min(this.chunk);
        let start = this.position;
        Poll::Ready(Ok(&this.data[start..start + take]))
    }

    fn consume(self: Pin<&mut Self>, amount: usize) {
        self.get_mut().position += amount;
    }
}

fn chunked(data: Vec<u8>, chunk: usize) -> Chunked {
    Chunked {
        data,
        position: 0,
        chunk,
    }
}

// ---------------------------------------------------------------------------
// parse_line: the decode boundary
// ---------------------------------------------------------------------------

#[test]
fn parse_line_never_panics_on_arbitrary_utf8() {
    let mut rng = Rng::new(0xA11C_0001);
    for _ in 0..5_000 {
        // The reader rejects invalid UTF-8 before parse_line sees it, so feed the
        // decoder every kind of valid string it can actually receive.
        let len = rng.below(160);
        let text = String::from_utf8_lossy(&rng.bytes(len)).into_owned();
        let decoded = std::panic::catch_unwind(AssertUnwindSafe(|| parse_line(&text)));
        assert!(decoded.is_ok(), "parse_line panicked on {text:?}");
    }

    // Arbitrary scalar values, not just the lossy replacement of random bytes.
    for _ in 0..5_000 {
        let text = rng.unicode(64);
        let decoded = std::panic::catch_unwind(AssertUnwindSafe(|| parse_line(&text)));
        assert!(decoded.is_ok(), "parse_line panicked on {text:?}");
    }
}

#[test]
fn parse_line_survives_pathological_json_nesting() {
    // serde_json's recursion limit rejects these as `Invalid` instead of
    // overflowing the stack; the assertion only requires that it not panic.
    for depth in [2usize, 128, 1_000] {
        let brackets = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        let line = format!("{{\"result\":{brackets}}}");
        assert!(matches!(parse_line(&line), WireLine::Invalid { .. }));
    }
}

#[test]
fn parse_line_round_trips_generated_results() {
    let mut rng = Rng::new(0xA11C_0002);
    for _ in 0..2_000 {
        let result = random_file_result(&mut rng);
        let line = format!(
            "{{\"id\":{},\"result\":{}}}",
            rng.below(1_000_000),
            serde_json::to_string(&result).expect("serialize result")
        );
        match parse_line(&line) {
            WireLine::Result(parsed) => assert_eq!(parsed, result, "line {line}"),
            other => panic!("expected a result for {line}, got {other:?}"),
        }
    }
}

#[test]
fn parse_line_round_trips_generated_errors_and_done() {
    let mut rng = Rng::new(0xA11C_0003);
    for _ in 0..2_000 {
        let error = random_error(&mut rng);
        let line = format!(
            "{{\"id\":{},\"error\":{}}}",
            rng.below(1_000_000),
            serde_json::to_string(&error).expect("serialize error")
        );
        match parse_line(&line) {
            WireLine::Error(parsed) => assert_eq!(parsed, error, "line {line}"),
            other => panic!("expected an error for {line}, got {other:?}"),
        }
    }

    for id in 0..100 {
        let line = format!("{{\"id\":{id},\"done\":true}}");
        assert_eq!(parse_line(&line), WireLine::Done);
    }
}

#[test]
fn parse_line_prefers_done_then_error_then_result() {
    // A hostile provider can put every discriminator on one line; the decoder
    // must pick a fixed precedence rather than getting confused.
    let line =
        r#"{"id":1,"done":true,"error":{"code":"x","message":"y"},"result":{"addresses":[]}}"#;
    assert_eq!(parse_line(line), WireLine::Done);

    let line = r#"{"id":1,"error":{"code":"x","message":"y"},"result":{"addresses":[]}}"#;
    match parse_line(line) {
        WireLine::Error(error) => assert_eq!(error.code, ErrorCode("x".to_owned())),
        other => panic!("expected an error, got {other:?}"),
    }
}

#[test]
fn parse_line_tolerates_unknown_fields() {
    let line =
        r#"{"id":1,"future":true,"result":{"provider":"p","addresses":[],"unknown":{"x":1}}}"#;
    match parse_line(line) {
        WireLine::Result(result) => assert_eq!(result.provider, "p"),
        other => panic!("expected a result, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// read_line_bounded: the bounded-read boundary
// ---------------------------------------------------------------------------

#[tokio::test]
async fn read_line_bounded_never_returns_more_than_its_bound() {
    let mut rng = Rng::new(0xB0D_0001);
    for _ in 0..3_000 {
        let len = rng.below(320);
        let data = rng.bytes(len);
        let max = rng.below(64);
        let chunk = 1 + rng.below(16);
        let mut reader = chunked(data, chunk);
        match read_line_bounded(&mut reader, max).await {
            Ok(Some(line)) => assert!(
                line.len() <= max,
                "returned a {}-byte line for a {max}-byte bound: {line:?}",
                line.len()
            ),
            Ok(None) => {}
            Err(WireReadError::TooLong { limit }) => assert_eq!(limit, max),
            // Random bytes are not always valid UTF-8; rejecting them is fine.
            Err(WireReadError::Io(_)) => {}
        }
    }
}

#[tokio::test]
async fn read_line_bounded_preserves_arbitrary_utf8_lines_across_chunks() {
    let mut rng = Rng::new(0xB0D_0002);
    for _ in 0..128 {
        let lines: Vec<String> = (0..1 + rng.below(4))
            .map(|_| random_line(&mut rng))
            .collect();
        let input = format!("{}\n", lines.join("\n")).into_bytes();
        for chunk in [1usize, 2, 3, 5, 8, 4_096] {
            let mut reader = chunked(input.clone(), chunk);
            let mut got = Vec::new();
            while let Some(line) = read_line_bounded(&mut reader, MAX_LINE_BYTES)
                .await
                .expect("read")
            {
                got.push(line);
            }
            assert_eq!(got, lines, "chunk={chunk}");
        }
    }
}

#[tokio::test]
async fn read_line_bounded_handles_multibyte_chars_split_across_chunks() {
    let line = "héllo—世界🌍";
    let mut input = line.as_bytes().to_vec();
    input.push(b'\n');
    for chunk in 1..=line.len() {
        let mut reader = chunked(input.clone(), chunk);
        assert_eq!(
            read_line_bounded(&mut reader, MAX_LINE_BYTES)
                .await
                .expect("read"),
            Some(line.to_owned()),
            "chunk={chunk}"
        );
    }
}

#[tokio::test]
async fn read_line_bounded_round_trips_generated_ndjson() {
    let mut rng = Rng::new(0xB0D_0003);
    for _ in 0..64 {
        let mut input = String::new();
        let mut expected = Vec::new();
        for _ in 0..1 + rng.below(5) {
            match rng.below(3) {
                0 => {
                    let result = random_file_result(&mut rng);
                    input.push_str(&format!(
                        "{{\"id\":1,\"result\":{}}}\n",
                        serde_json::to_string(&result).expect("serialize result")
                    ));
                    expected.push(WireLine::Result(result));
                }
                1 => {
                    let error = random_error(&mut rng);
                    input.push_str(&format!(
                        "{{\"id\":1,\"error\":{}}}\n",
                        serde_json::to_string(&error).expect("serialize error")
                    ));
                    expected.push(WireLine::Error(error));
                }
                _ => {
                    input.push_str("{\"id\":1,\"done\":true}\n");
                    expected.push(WireLine::Done);
                }
            }
        }

        for chunk in [1usize, 2, 7, 64, 4_096] {
            let mut reader = chunked(input.clone().into_bytes(), chunk);
            let mut got = Vec::new();
            while let Some(line) = read_line_bounded(&mut reader, MAX_LINE_BYTES)
                .await
                .expect("read")
            {
                got.push(parse_line(&line));
            }
            assert_eq!(got, expected, "chunk={chunk}");
        }
    }
}

#[tokio::test]
async fn read_line_bounded_strips_crlf_and_reads_a_final_unterminated_line() {
    let mut reader = chunked(b"alpha\r\nbeta".to_vec(), 1);
    assert_eq!(
        read_line_bounded(&mut reader, 64).await.expect("read"),
        Some("alpha".to_owned())
    );
    assert_eq!(
        read_line_bounded(&mut reader, 64).await.expect("read"),
        Some("beta".to_owned())
    );
    assert_eq!(
        read_line_bounded(&mut reader, 64).await.expect("read"),
        None
    );
}

#[tokio::test]
async fn read_line_bounded_rejects_invalid_utf8() {
    let mut reader = chunked(vec![0xFF, 0xFE, b'\n'], 2);
    assert!(matches!(
        read_line_bounded(&mut reader, 64).await,
        Err(WireReadError::Io(_))
    ));
}

// ---------------------------------------------------------------------------
// The encoders always emit exactly one line
// ---------------------------------------------------------------------------

#[test]
fn encoded_requests_are_single_lines() {
    let search = encode_request(3, &FileQuery::new("x")).expect("encode search");
    assert!(search.ends_with('\n'));
    assert!(!search.trim_end_matches('\n').contains('\n'));

    let capabilities = encode_capabilities_request(4).expect("encode capabilities");
    assert!(capabilities.ends_with('\n'));
    assert!(!capabilities.trim_end_matches('\n').contains('\n'));
}
