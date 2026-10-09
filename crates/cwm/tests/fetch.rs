#![cfg(feature = "fetch")]

use cwm::{sha256_hex, Address, CommandFetcher, FetchError, FetchRegistry};
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

fn temp_dest(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("cwm-fetch-{}-{tag}", std::process::id()))
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
