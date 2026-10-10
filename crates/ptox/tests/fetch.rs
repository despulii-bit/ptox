#![cfg(feature = "fetch")]

use ptox::{
    sha256_file, sha256_hex, Address, CommandFetcher, FetchError, FetchRegistry, HttpFetcher,
    RetryPolicy,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn serve_once(body: &'static [u8]) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buffer = [0u8; 2048];
            let _ = socket.read(&mut buffer).await;
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = socket.write_all(header.as_bytes()).await;
            let _ = socket.write_all(body).await;
            let _ = socket.flush().await;
        }
    });
    format!("http://{addr}/file")
}

/// A server that promises more bytes than it sends, so the client sees a
/// mid-stream transport error.
async fn serve_truncated() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buffer = [0u8; 2048];
            let _ = socket.read(&mut buffer).await;
            let header = "HTTP/1.1 200 OK\r\nContent-Length: 1024\r\nConnection: close\r\n\r\n";
            let _ = socket.write_all(header.as_bytes()).await;
            let _ = socket.write_all(b"short").await;
            let _ = socket.flush().await;
        }
    });
    format!("http://{addr}/file")
}

/// One canned reply for [`serve_script`].
enum Reply {
    /// A non-2xx status with an empty body.
    Status(u16),
    /// A complete 200 response with this body.
    Body(&'static [u8]),
    /// A 200 response that promises 1024 bytes but sends 5 (mid-stream error).
    Truncated,
}

/// Serve one reply per accepted connection, in order, and count the connections
/// actually accepted. Lets a test drive a flaky endpoint deterministically.
async fn serve_script(replies: Vec<Reply>) -> (String, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = hits.clone();
    tokio::spawn(async move {
        for reply in replies {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            counted.fetch_add(1, Ordering::SeqCst);
            let mut buffer = [0u8; 2048];
            let _ = socket.read(&mut buffer).await;
            match reply {
                Reply::Status(code) => {
                    let response = format!(
                        "HTTP/1.1 {code} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                }
                Reply::Body(body) => {
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(header.as_bytes()).await;
                    let _ = socket.write_all(body).await;
                }
                Reply::Truncated => {
                    let header =
                        "HTTP/1.1 200 OK\r\nContent-Length: 1024\r\nConnection: close\r\n\r\n";
                    let _ = socket.write_all(header.as_bytes()).await;
                    let _ = socket.write_all(b"short").await;
                }
            }
            let _ = socket.flush().await;
        }
    });
    (format!("http://{addr}/file"), hits)
}

/// An `HttpFetcher` registry with bounded retries and no sleeping, so retry
/// tests stay fast but exercise the real loop.
fn retrying_registry(attempts: u32) -> FetchRegistry {
    let mut registry = FetchRegistry::new();
    registry.register(Arc::new(
        HttpFetcher::new().with_retry(RetryPolicy::new(attempts).with_base_delay(Duration::ZERO)),
    ));
    registry
}

fn temp_dest(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("ptox-fetch-{}-{tag}", std::process::id()))
}

#[tokio::test]
async fn unsupported_scheme_is_rejected() {
    let registry = FetchRegistry::with_defaults();
    let address = Address::new("magnet", "magnet:?xt=urn:btih:abc");
    let error = registry
        .fetch(&address, temp_dest("unsupported"), None)
        .await
        .expect_err("should reject magnet");
    assert!(matches!(error, FetchError::Unsupported(scheme) if scheme == "magnet"));
}

#[tokio::test]
async fn http_fetches_and_verifies() {
    let body: &'static [u8] = b"hello world";
    let url = serve_once(body).await;
    let dest = temp_dest("http");
    let registry = FetchRegistry::with_defaults();

    let fetched = registry
        .fetch(
            &Address::new("http", url),
            dest.clone(),
            Some(sha256_hex(body)),
        )
        .await
        .expect("fetch");

    assert_eq!(fetched.bytes, body.len() as u64);
    assert_eq!(std::fs::read(&dest).expect("read"), body);
    let _ = std::fs::remove_file(&dest);
}

#[tokio::test]
async fn http_hash_mismatch_removes_the_partial_file() {
    let body: &'static [u8] = b"hello world";
    let url = serve_once(body).await;
    let dest = temp_dest("mismatch");
    let registry = FetchRegistry::with_defaults();

    let error = registry
        .fetch(
            &Address::new("http", url),
            dest.clone(),
            Some("00".to_owned()),
        )
        .await
        .expect_err("hash mismatch");

    assert!(matches!(error, FetchError::HashMismatch { .. }));
    assert!(
        !dest.exists(),
        "destination must not be created on mismatch"
    );
}

#[tokio::test]
async fn command_fetcher_delegates_a_scheme() {
    let source = temp_dest("source");
    std::fs::write(&source, b"payload").expect("write source");
    let dest = temp_dest("command");

    let mut registry = FetchRegistry::new();
    registry.register(Arc::new(CommandFetcher::new(
        "copy",
        vec!["copy".to_owned()],
        "cp",
        vec![source.to_string_lossy().into_owned(), "{dest}".to_owned()],
    )));

    let fetched = registry
        .fetch(&Address::new("copy", "ignored"), dest.clone(), None)
        .await
        .expect("fetch");

    assert_eq!(std::fs::read(&dest).expect("read"), b"payload");
    assert_eq!(fetched.bytes, 7);
    let _ = std::fs::remove_file(&source);
    let _ = std::fs::remove_file(&dest);
}

#[tokio::test]
async fn command_fetcher_errors_when_it_creates_nothing() {
    let dest = temp_dest("noop-command");
    let mut registry = FetchRegistry::new();
    registry.register(Arc::new(CommandFetcher::new(
        "noop",
        vec!["noop".to_owned()],
        "true",
        Vec::new(),
    )));

    let error = registry
        .fetch(&Address::new("noop", "ignored"), dest.clone(), None)
        .await
        .expect_err("a command that creates nothing must fail");

    assert!(matches!(error, FetchError::Command(_)), "got {error:?}");
    assert!(!dest.exists());
}

#[tokio::test]
async fn http_stream_error_removes_the_partial_file() {
    let url = serve_truncated().await;
    let dest = temp_dest("truncated");
    let registry = FetchRegistry::with_defaults();

    let error = registry
        .fetch(&Address::new("http", url), dest.clone(), None)
        .await
        .expect_err("a truncated body must fail");

    assert!(matches!(error, FetchError::Network(_)), "got {error:?}");
    assert!(!dest.exists(), "destination must not exist on failure");
    let part = dest.with_file_name(format!(
        "{}.part",
        dest.file_name().expect("name").to_string_lossy()
    ));
    assert!(!part.exists(), "partial file must be removed on failure");
}

#[tokio::test]
async fn http_retry_is_off_by_default() {
    let body: &'static [u8] = b"unused";
    let (url, hits) = serve_script(vec![Reply::Status(503), Reply::Body(body)]).await;
    let dest = temp_dest("no-retry-default");
    let registry = FetchRegistry::with_defaults();

    let error = registry
        .fetch(&Address::new("http", url), dest.clone(), None)
        .await
        .expect_err("a 503 must fail without retries");

    assert!(matches!(error, FetchError::Network(_)), "got {error:?}");
    assert_eq!(hits.load(Ordering::SeqCst), 1, "default must not retry");
    assert!(!dest.exists());
}

#[tokio::test]
async fn http_retries_transient_status_then_succeeds() {
    let body: &'static [u8] = b"recovered payload";
    let (url, hits) = serve_script(vec![
        Reply::Status(503),
        Reply::Status(429),
        Reply::Body(body),
    ])
    .await;
    let dest = temp_dest("retry-success");
    let registry = retrying_registry(3);

    let fetched = registry
        .fetch(
            &Address::new("http", url),
            dest.clone(),
            Some(sha256_hex(body)),
        )
        .await
        .expect("third attempt succeeds");

    assert_eq!(fetched.bytes, body.len() as u64);
    assert_eq!(std::fs::read(&dest).expect("read"), body);
    assert_eq!(
        hits.load(Ordering::SeqCst),
        3,
        "two retries after the first failure"
    );
    let _ = std::fs::remove_file(&dest);
}

#[tokio::test]
async fn http_retry_exhaustion_reports_the_last_error() {
    let (url, hits) = serve_script(vec![
        Reply::Status(503),
        Reply::Status(503),
        Reply::Status(503),
    ])
    .await;
    let dest = temp_dest("retry-exhausted");
    let registry = retrying_registry(3);

    let error = registry
        .fetch(&Address::new("http", url), dest.clone(), None)
        .await
        .expect_err("every attempt fails");

    assert!(matches!(error, FetchError::Network(_)), "got {error:?}");
    assert_eq!(
        hits.load(Ordering::SeqCst),
        3,
        "exactly max_attempts requests"
    );
    assert!(!dest.exists());
}

#[tokio::test]
async fn http_does_not_retry_permanent_client_errors() {
    let body: &'static [u8] = b"unused";
    let (url, hits) = serve_script(vec![Reply::Status(404), Reply::Body(body)]).await;
    let dest = temp_dest("no-retry-404");
    let registry = retrying_registry(3);

    let error = registry
        .fetch(&Address::new("http", url), dest.clone(), None)
        .await
        .expect_err("404 is permanent");

    assert!(matches!(error, FetchError::Network(_)), "got {error:?}");
    assert_eq!(hits.load(Ordering::SeqCst), 1, "4xx must not be retried");
    assert!(!dest.exists());
}

#[tokio::test]
async fn http_retry_resets_a_partial_download() {
    let body: &'static [u8] = b"complete body after reset";
    let (url, hits) = serve_script(vec![Reply::Truncated, Reply::Body(body)]).await;
    let dest = temp_dest("retry-partial");
    let registry = retrying_registry(2);

    let fetched = registry
        .fetch(
            &Address::new("http", url),
            dest.clone(),
            Some(sha256_hex(body)),
        )
        .await
        .expect("retry after a truncated first attempt");

    assert_eq!(fetched.bytes, body.len() as u64);
    assert_eq!(std::fs::read(&dest).expect("read"), body);
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    let _ = std::fs::remove_file(&dest);
}

#[tokio::test]
async fn http_reports_cumulative_progress() {
    let body: &'static [u8] = b"hello progress world";
    let url = serve_once(body).await;
    let dest = temp_dest("progress");
    let registry = FetchRegistry::with_defaults();
    let seen = std::sync::Mutex::new(Vec::new());

    let fetched = registry
        .fetch_with_progress(
            &Address::new("http", url),
            dest.clone(),
            Some(sha256_hex(body)),
            &|bytes: u64| seen.lock().expect("lock").push(bytes),
        )
        .await
        .expect("fetch");

    let seen = seen.into_inner().expect("lock");
    assert!(!seen.is_empty(), "progress must be reported at least once");
    assert_eq!(fetched.bytes, body.len() as u64);
    assert_eq!(
        *seen.last().expect("last"),
        body.len() as u64,
        "final progress must equal the body length"
    );
    assert!(
        seen.windows(2).all(|pair| pair[0] <= pair[1]),
        "progress must never go backwards: {seen:?}"
    );
    let _ = std::fs::remove_file(&dest);
}

#[tokio::test]
async fn engine_fetch_with_progress_reports() {
    use ptox::{Engine, MergedResult};

    let body: &'static [u8] = b"engine progress";
    let url = serve_once(body).await;
    let dest = temp_dest("engine-progress");
    let engine = Engine::new(Vec::new());
    let result = MergedResult {
        addresses: vec![Address::new("http", url)],
        ..Default::default()
    };
    let seen = std::sync::Mutex::new(Vec::new());

    let fetched = engine
        .fetch_with_progress(&result, dest.clone(), &|bytes: u64| {
            seen.lock().expect("lock").push(bytes)
        })
        .await
        .expect("fetch");

    assert_eq!(fetched.bytes, body.len() as u64);
    assert_eq!(
        *seen.into_inner().expect("lock").last().expect("last"),
        body.len() as u64
    );
    let _ = std::fs::remove_file(&dest);
}

#[tokio::test]
async fn command_fetcher_reports_final_progress() {
    let source = temp_dest("progress-source");
    std::fs::write(&source, b"payload").expect("write source");
    let dest = temp_dest("progress-command");

    let mut registry = FetchRegistry::new();
    registry.register(Arc::new(CommandFetcher::new(
        "copy",
        vec!["copy".to_owned()],
        "cp",
        vec![source.to_string_lossy().into_owned(), "{dest}".to_owned()],
    )));

    let seen = std::sync::Mutex::new(Vec::new());
    let fetched = registry
        .fetch_with_progress(
            &Address::new("copy", "ignored"),
            dest.clone(),
            None,
            &|bytes: u64| seen.lock().expect("lock").push(bytes),
        )
        .await
        .expect("fetch");

    assert_eq!(std::fs::read(&dest).expect("read"), b"payload");
    assert_eq!(fetched.bytes, 7);
    let seen = seen.into_inner().expect("lock");
    assert!(!seen.is_empty(), "command fetcher must report progress");
    assert_eq!(
        *seen.last().expect("last"),
        7,
        "final progress must equal the fetched size"
    );
    assert!(
        seen.windows(2).all(|pair| pair[0] <= pair[1]),
        "progress must never go backwards: {seen:?}"
    );
    let _ = std::fs::remove_file(&source);
    let _ = std::fs::remove_file(&dest);
}

/// Proves progress is polled *while* the command runs, not just after it exits:
/// the command waits for a marker that only the progress callback creates.
#[tokio::test]
async fn command_fetcher_polls_progress_while_the_command_runs() {
    let dest = temp_dest("command-poll");
    let gate = temp_dest("command-poll-gate");
    let _ = std::fs::remove_file(&dest);
    let _ = std::fs::remove_file(&gate);

    let script = "printf '1234' > \"$1\"; while [ ! -e \"$2\" ]; do sleep 0.01; done; printf '5678' >> \"$1\"";
    let mut registry = FetchRegistry::new();
    registry.register(Arc::new(CommandFetcher::new(
        "gated",
        vec!["gated".to_owned()],
        "sh",
        vec![
            "-c".to_owned(),
            script.to_owned(),
            "sh".to_owned(),
            "{dest}".to_owned(),
            gate.to_string_lossy().into_owned(),
        ],
    )));

    let seen = std::sync::Mutex::new(Vec::new());
    let fetched = registry
        .fetch_with_progress(
            &Address::new("gated", "ignored"),
            dest.clone(),
            None,
            &|bytes: u64| {
                seen.lock().expect("lock").push(bytes);
                // Unblock the command once the first partial size is observed.
                if bytes == 4 {
                    let _ = std::fs::write(&gate, b"go");
                }
            },
        )
        .await
        .expect("fetch");

    assert_eq!(fetched.bytes, 8);
    let seen = seen.into_inner().expect("lock");
    assert!(
        seen.contains(&4),
        "the partial size must be reported while the command runs: {seen:?}"
    );
    assert_eq!(*seen.last().expect("last"), 8);
    let _ = std::fs::remove_file(&dest);
    let _ = std::fs::remove_file(&gate);
}

#[tokio::test]
async fn sha256_file_streams_and_matches_in_memory_hash() {
    let path = temp_dest("sha-file");
    // Larger than the streaming buffer, so multiple reads are exercised.
    let body = vec![7u8; 200_000];
    std::fs::write(&path, &body).expect("write");

    let digest = sha256_file(&path).await.expect("hash");

    assert_eq!(digest, sha256_hex(&body));
    let _ = std::fs::remove_file(&path);
}

#[cfg(feature = "ipfs")]
use ptox::IpfsGatewayFetcher;

/// A stub HTTP gateway that accepts one request, records its path, and answers
/// with `body`.
#[cfg(feature = "ipfs")]
async fn serve_gateway(
    body: &'static [u8],
) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let paths = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = paths.clone();
    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buffer = [0u8; 2048];
            let read = socket.read(&mut buffer).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&buffer[..read]);
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or_default()
                .to_owned();
            recorded.lock().expect("lock").push(path);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = socket.write_all(header.as_bytes()).await;
            let _ = socket.write_all(body).await;
            let _ = socket.flush().await;
        }
    });
    (format!("http://{addr}"), paths)
}

#[cfg(feature = "ipfs")]
#[tokio::test]
async fn ipfs_gateway_rewrites_uri_and_fetches() {
    let body: &'static [u8] = b"ipfs payload";
    let (gateway, paths) = serve_gateway(body).await;
    let dest = temp_dest("ipfs");
    let mut registry = FetchRegistry::new();
    registry.register(Arc::new(IpfsGatewayFetcher::new(gateway)));

    let fetched = registry
        .fetch(
            &Address::new("ipfs", "ipfs://bafyabc/docs/report.pdf"),
            dest.clone(),
            Some(sha256_hex(body)),
        )
        .await
        .expect("fetch");

    assert_eq!(fetched.bytes, body.len() as u64);
    assert_eq!(std::fs::read(&dest).expect("read"), body);
    let recorded = paths.lock().expect("lock").clone();
    assert_eq!(recorded.len(), 1, "one gateway request expected");
    assert_eq!(recorded[0], "/ipfs/bafyabc/docs/report.pdf");
    let _ = std::fs::remove_file(&dest);
}

#[cfg(feature = "ipfs")]
#[tokio::test]
async fn ipfs_gateway_reports_progress_through_delegation() {
    let body: &'static [u8] = b"ipfs progress payload";
    let (gateway, _paths) = serve_gateway(body).await;
    let dest = temp_dest("ipfs-progress");
    let mut registry = FetchRegistry::new();
    registry.register(Arc::new(IpfsGatewayFetcher::new(gateway)));
    let seen = std::sync::Mutex::new(Vec::new());

    let fetched = registry
        .fetch_with_progress(
            &Address::new("ipfs", "ipfs://bafyprogress"),
            dest.clone(),
            None,
            &|bytes: u64| seen.lock().expect("lock").push(bytes),
        )
        .await
        .expect("fetch");

    assert_eq!(fetched.bytes, body.len() as u64);
    assert_eq!(
        *seen.into_inner().expect("lock").last().expect("last"),
        body.len() as u64
    );
    let _ = std::fs::remove_file(&dest);
}

#[cfg(feature = "ipfs")]
#[tokio::test]
async fn ipfs_gateway_rejects_a_uri_without_a_cid() {
    let dest = temp_dest("ipfs-empty");
    let mut registry = FetchRegistry::new();
    registry.register(Arc::new(IpfsGatewayFetcher::new("http://127.0.0.1:1")));

    let error = registry
        .fetch(&Address::new("ipfs", "ipfs://"), dest.clone(), None)
        .await
        .expect_err("a cid-less ipfs URI must fail");

    assert!(
        matches!(error, FetchError::InvalidAddress(_)),
        "got {error:?}"
    );
    assert!(!dest.exists());
}

#[cfg(feature = "ipfs")]
#[tokio::test]
async fn defaults_resolve_ipfs_when_the_feature_is_on() {
    let registry = FetchRegistry::with_defaults();
    assert!(
        registry.fetcher_for("ipfs").is_some(),
        "with_defaults must include the ipfs gateway fetcher"
    );
}

#[test]
fn preferred_addresses_are_ordered_by_scheme_then_registration() {
    use ptox::{preferred_addresses, MergedResult};

    let result = MergedResult {
        addresses: vec![
            Address::new("magnet", "magnet:?xt=urn:btih:abc"),
            Address::new("http", "http://mirror-a/file"),
            Address::new("https", "https://mirror-b/file"),
            Address::new("http", "http://mirror-c/file"),
        ],
        ..Default::default()
    };

    let ordered: Vec<&str> = preferred_addresses(&result)
        .into_iter()
        .map(|address| address.uri.as_str())
        .collect();
    assert_eq!(
        ordered,
        vec![
            "https://mirror-b/file",
            "http://mirror-a/file",
            "http://mirror-c/file",
            "magnet:?xt=urn:btih:abc",
        ]
    );
}

#[tokio::test]
async fn fetch_merged_falls_back_to_a_working_address() {
    use ptox::MergedResult;

    let body: &'static [u8] = b"mirror payload";
    let good = serve_once(body).await;
    let result = MergedResult {
        addresses: vec![
            // Preferred by scheme (`https`), but the port is closed.
            Address::new("https", "http://127.0.0.1:1/dead"),
            Address::new("http", good.clone()),
        ],
        ..Default::default()
    };
    let dest = temp_dest("failover");
    let registry = FetchRegistry::with_defaults();

    let fetched = registry
        .fetch_merged(&result, dest.clone())
        .await
        .expect("fallback to the second address");

    assert_eq!(fetched.bytes, body.len() as u64);
    assert_eq!(fetched.address.uri, good, "the winning address is reported");
    assert_eq!(std::fs::read(&dest).expect("read"), body);
    let _ = std::fs::remove_file(&dest);
}

#[tokio::test]
async fn fetch_merged_reports_all_failures() {
    use ptox::MergedResult;

    let result = MergedResult {
        addresses: vec![
            Address::new("http", "http://127.0.0.1:1/one"),
            Address::new("http", "http://127.0.0.1:1/two"),
        ],
        ..Default::default()
    };
    let registry = FetchRegistry::with_defaults();

    let error = registry
        .fetch_merged(&result, temp_dest("all-fail"))
        .await
        .expect_err("every address is dead");

    match error {
        FetchError::AllFailed { count, errors } => {
            assert_eq!(count, 2);
            assert_eq!(errors.len(), 2, "each failure is preserved");
        }
        other => panic!("expected AllFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn fetch_merged_failover_keeps_progress_monotonic() {
    use ptox::MergedResult;

    let body: &'static [u8] = b"mirror payload";
    let truncated = serve_truncated().await;
    let good = serve_once(body).await;
    let result = MergedResult {
        addresses: vec![
            Address::new("http", truncated),
            Address::new("http", good.clone()),
        ],
        ..Default::default()
    };
    let dest = temp_dest("failover-progress");
    let registry = FetchRegistry::with_defaults();
    let seen = std::sync::Mutex::new(Vec::new());

    let fetched = registry
        .fetch_merged_with_progress(&result, dest.clone(), &|bytes: u64| {
            seen.lock().expect("lock").push(bytes)
        })
        .await
        .expect("fallback");

    assert_eq!(fetched.address.uri, good);
    let seen = seen.into_inner().expect("lock");
    assert!(
        seen.windows(2).all(|pair| pair[0] <= pair[1]),
        "progress must not regress across failover: {seen:?}"
    );
    assert_eq!(
        *seen.last().expect("last"),
        body.len() as u64,
        "the final report is the winning address's size"
    );
    let _ = std::fs::remove_file(&dest);
}

#[tokio::test]
async fn fetch_merged_with_no_addresses_is_a_no_address_error() {
    use ptox::MergedResult;

    let registry = FetchRegistry::with_defaults();
    let error = registry
        .fetch_merged(&MergedResult::default(), temp_dest("no-address"))
        .await
        .expect_err("no addresses");
    assert!(matches!(error, FetchError::NoAddress));
}

#[cfg(feature = "file")]
use ptox::FileFetcher;

#[cfg(feature = "file")]
#[tokio::test]
async fn file_fetcher_copies_and_verifies() {
    use ptox::MergedResult;

    let body: &[u8] = b"local payload";
    let source = temp_dest("file-source");
    std::fs::write(&source, body).expect("write source");
    let dest = temp_dest("file-dest");
    let registry = FetchRegistry::with_defaults();

    let fetched = registry
        .fetch(
            &Address::new("file", format!("file://{}", source.display())),
            dest.clone(),
            Some(sha256_hex(body)),
        )
        .await
        .expect("fetch");

    assert_eq!(fetched.bytes, body.len() as u64);
    assert_eq!(std::fs::read(&dest).expect("read"), body);
    assert_eq!(fetched.sha256.as_deref(), Some(sha256_hex(body).as_str()));
    // The scheme is also resolvable through the merged-result path.
    let result = MergedResult {
        addresses: vec![Address::new("file", format!("file://{}", source.display()))],
        ..Default::default()
    };
    let merged_dest = temp_dest("file-merged");
    let fetched = registry
        .fetch_merged(&result, merged_dest.clone())
        .await
        .expect("fetch merged");
    assert_eq!(fetched.address.scheme, "file");
    assert_eq!(std::fs::read(&merged_dest).expect("read"), body);

    let _ = std::fs::remove_file(&source);
    let _ = std::fs::remove_file(&dest);
    let _ = std::fs::remove_file(&merged_dest);
}

#[cfg(feature = "file")]
#[tokio::test]
async fn file_fetcher_hash_mismatch_leaves_nothing() {
    let source = temp_dest("file-mismatch-source");
    std::fs::write(&source, b"local payload").expect("write source");
    let dest = temp_dest("file-mismatch-dest");
    let registry = FetchRegistry::with_defaults();

    let error = registry
        .fetch(
            &Address::new("file", format!("file://{}", source.display())),
            dest.clone(),
            Some("00".to_owned()),
        )
        .await
        .expect_err("hash mismatch");

    assert!(matches!(error, FetchError::HashMismatch { .. }));
    assert!(!dest.exists(), "no destination on mismatch");
    let part = dest.with_file_name(format!(
        "{}.part",
        dest.file_name().expect("name").to_string_lossy()
    ));
    assert!(!part.exists(), "no partial file on mismatch");
    let _ = std::fs::remove_file(&source);
}

#[cfg(feature = "file")]
#[tokio::test]
async fn file_fetcher_reports_monotonic_progress() {
    let body = vec![9u8; 200_000];
    let source = temp_dest("file-progress-source");
    std::fs::write(&source, &body).expect("write source");
    let dest = temp_dest("file-progress-dest");
    let registry = FetchRegistry::with_defaults();
    let seen = std::sync::Mutex::new(Vec::new());

    let fetched = registry
        .fetch_with_progress(
            &Address::new("file", format!("file://{}", source.display())),
            dest.clone(),
            None,
            &|bytes: u64| seen.lock().expect("lock").push(bytes),
        )
        .await
        .expect("fetch");

    assert_eq!(fetched.bytes, body.len() as u64);
    let seen = seen.into_inner().expect("lock");
    assert!(
        seen.len() > 1,
        "a multi-chunk file must report more than once: {seen:?}"
    );
    assert!(
        seen.windows(2).all(|pair| pair[0] <= pair[1]),
        "progress must never go backwards: {seen:?}"
    );
    assert_eq!(*seen.last().expect("last"), body.len() as u64);
    let _ = std::fs::remove_file(&source);
    let _ = std::fs::remove_file(&dest);
}

#[cfg(feature = "file")]
#[tokio::test]
async fn file_fetcher_enforces_allowed_roots() {
    let parent = std::env::temp_dir().join(format!("ptox-file-roots-{}", std::process::id()));
    let allowed = parent.join("allowed");
    let outside = parent.join("outside");
    std::fs::create_dir_all(&allowed).expect("mkdir allowed");
    std::fs::create_dir_all(&outside).expect("mkdir outside");
    let inside_source = allowed.join("inside.txt");
    let outside_source = outside.join("outside.txt");
    std::fs::write(&inside_source, b"inside").expect("write inside");
    std::fs::write(&outside_source, b"outside").expect("write outside");

    let mut registry = FetchRegistry::new();
    registry.register(Arc::new(FileFetcher::with_allowed_roots(vec![
        allowed.clone()
    ])));

    // Inside the root is allowed.
    let dest = parent.join("inside-dest.txt");
    registry
        .fetch(
            &Address::new("file", format!("file://{}", inside_source.display())),
            dest.clone(),
            None,
        )
        .await
        .expect("inside the root");
    assert_eq!(std::fs::read(&dest).expect("read"), b"inside");

    // Outside the root is refused, and nothing is written.
    let denied_dest = parent.join("denied-dest.txt");
    let error = registry
        .fetch(
            &Address::new("file", format!("file://{}", outside_source.display())),
            denied_dest.clone(),
            None,
        )
        .await
        .expect_err("outside the root must be denied");
    assert!(
        matches!(error, FetchError::AccessDenied(_)),
        "got {error:?}"
    );
    assert!(!denied_dest.exists());

    let _ = std::fs::remove_dir_all(&parent);
}

#[cfg(feature = "file")]
#[tokio::test]
async fn file_fetcher_rejects_malformed_uris() {
    let registry = FetchRegistry::with_defaults();
    for uri in ["file://", "file://remote/share", "file:///tmp/a%2"] {
        let error = registry
            .fetch(&Address::new("file", uri), temp_dest("file-bad"), None)
            .await
            .expect_err("malformed file URI");
        assert!(
            matches!(error, FetchError::InvalidAddress(_)),
            "`{uri}` -> {error:?}"
        );
    }
}

#[cfg(feature = "file")]
#[tokio::test]
async fn file_fetcher_rejects_a_directory() {
    let dest = temp_dest("file-directory");
    let registry = FetchRegistry::with_defaults();
    let error = registry
        .fetch(
            &Address::new("file", format!("file://{}", std::env::temp_dir().display())),
            dest.clone(),
            None,
        )
        .await
        .expect_err("a directory is not a regular file");
    assert!(matches!(error, FetchError::Io(_)), "got {error:?}");
    assert!(!dest.exists());
}

#[cfg(feature = "file")]
#[tokio::test]
async fn file_fetcher_decodes_percent_escapes_end_to_end() {
    let dir = std::env::temp_dir().join(format!("ptox-file-escape-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let source = dir.join("report final.txt");
    std::fs::write(&source, b"escaped payload").expect("write source");
    let dest = dir.join("dest.txt");
    let registry = FetchRegistry::with_defaults();

    let fetched = registry
        .fetch(
            &Address::new(
                "file",
                format!("file://{}/report%20final.txt", dir.display()),
            ),
            dest.clone(),
            None,
        )
        .await
        .expect("fetch via percent-encoded URI");

    assert_eq!(fetched.bytes, 15);
    assert_eq!(std::fs::read(&dest).expect("read"), b"escaped payload");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "file")]
#[test]
fn defaults_resolve_file_when_the_feature_is_on() {
    let registry = FetchRegistry::with_defaults();
    assert!(
        registry.fetcher_for("file").is_some(),
        "with_defaults must include the file fetcher"
    );
}
