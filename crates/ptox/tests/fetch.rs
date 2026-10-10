#![cfg(feature = "fetch")]

use ptox::{sha256_file, sha256_hex, Address, CommandFetcher, FetchError, FetchRegistry};
use std::path::PathBuf;
use std::sync::Arc;
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
