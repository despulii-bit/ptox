//! An end-to-end, **offline** client example.
//!
//! A client is providers in, ranked results and fetched files out. This example
//! assembles the whole path with no network at all:
//!
//! 1. an in-process [`SearchProvider`] over a fixed catalog;
//! 2. an [`Engine`] with a fake [`Fetcher`] registered for the `mem` scheme;
//! 3. a ranked [`Engine::search`]; and
//! 4. an [`Engine::fetch`] of the first hit to a temporary file, with sha256
//!    verification inherited from the fetch fabric.
//!
//! Run it — no sockets, deterministic bytes:
//!
//! ```sh
//! cargo run -p ptox --example client
//! ```
//!
//! See `docs/building-a-client.md` for the guide this mirrors, and
//! `crates/ptox/examples/rust_provider.rs` for the provider side.

use async_trait::async_trait;
use futures::stream;
use ptox::{
    sha256_hex, suggest_filename, verify_sha256, Address, Engine, FetchError, FetchRegistry,
    FetchRequest, Fetched, Fetcher, FileQuery, FileResult, HashAlgos, ResultStream, SearchProvider,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// The bytes the fake network serves for the one file in its catalog.
const REPORT: &[u8] = b"fake annual report contents\n";
/// The `mem://` address of that file.
const REPORT_URI: &str = "mem://annual-report-2024.txt";
/// The provider's stable name, used for attribution and ranking.
const PROVIDER: &str = "fake-index";

/// A tiny in-process index: a fixed catalog, filtered by a substring of the
/// query text. A real provider would talk to a network here.
struct FakeIndex {
    files: Vec<FileResult>,
}

impl SearchProvider for FakeIndex {
    fn name(&self) -> &str {
        PROVIDER
    }

    fn search(&self, query: &FileQuery) -> ResultStream<'_> {
        // Clone what the stream needs so it owns its inputs and does not borrow
        // `query` (the trait ties the returned stream's lifetime to `&self`).
        let needle = query.text.to_lowercase();
        let hits: Vec<FileResult> = self
            .files
            .iter()
            .filter(|file| {
                file.title
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(&needle)
            })
            .cloned()
            .collect();
        Box::pin(stream::iter(hits.into_iter().map(Ok)))
    }
}

/// A fake transport keyed by URI: it writes canned bytes, no sockets, and still
/// honors the fetch fabric's sha256 check. Register it for the `mem` scheme to
/// keep the whole example offline.
struct MemoryFetcher {
    schemes: Vec<String>,
    content: HashMap<String, Vec<u8>>,
}

impl MemoryFetcher {
    fn new(content: HashMap<String, Vec<u8>>) -> Self {
        Self {
            schemes: vec!["mem".to_owned()],
            content,
        }
    }
}

#[async_trait]
impl Fetcher for MemoryFetcher {
    fn name(&self) -> &str {
        "memory"
    }

    fn schemes(&self) -> &[String] {
        &self.schemes
    }

    async fn fetch(&self, request: &FetchRequest) -> Result<Fetched, FetchError> {
        let body = self.content.get(&request.address.uri).ok_or_else(|| {
            FetchError::Network(format!("no bytes for `{}`", request.address.uri))
        })?;
        if let Some(parent) = request.destination.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&request.destination, body).await?;
        let sha256 = sha256_hex(body);
        verify_sha256(request.expected_sha256.as_deref(), &sha256)?;
        Ok(Fetched {
            path: request.destination.clone(),
            bytes: body.len() as u64,
            sha256: Some(sha256),
            address: request.address.clone(),
        })
    }
}

/// Build the one-file catalog and an `Engine` wired to the fake fetcher. Used by
/// both `main` and the tests so they exercise exactly the same client.
fn build_engine() -> Engine {
    let mut hashes = HashAlgos::new();
    hashes.insert("sha256".to_owned(), sha256_hex(REPORT));

    let mut file = FileResult::new(
        // Attribution is host-owned (`fanout` stamps it); a Rust provider
        // leaves it empty.
        String::new(),
        vec![Address::new("mem", REPORT_URI)],
    );
    file.title = Some("annual report 2024.txt".to_owned());
    file.size = Some(REPORT.len() as u64);
    file.mimetype = Some("text/plain".to_owned());
    file.hashes = Some(hashes);

    // A registry with *only* the fake scheme: no network fetchers are reachable.
    let mut registry = FetchRegistry::new();
    registry.register(Arc::new(MemoryFetcher::new(HashMap::from([(
        REPORT_URI.to_owned(),
        REPORT.to_vec(),
    )]))));

    let provider: Arc<dyn SearchProvider> = Arc::new(FakeIndex { files: vec![file] });
    Engine::new(vec![provider]).with_fetchers(registry)
}

/// A per-process scratch directory so parallel test runs do not collide.
fn scratch_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("ptox-client-{}-{tag}", std::process::id()))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = build_engine();

    // 1. Search: the engine runs the provider, validates, attributes, ranks, and
    //    deduplicates the hits.
    let report = engine.search(FileQuery::new("annual report")).await;
    println!("{} result(s):", report.results.len());
    for (index, result) in report.results.iter().enumerate() {
        println!(
            "{:>3}. {} [via {}]",
            index + 1,
            result.title.as_deref().unwrap_or("<untitled>"),
            result.providers.join(", ")
        );
    }
    println!(
        "{} provider error(s), {} skipped",
        report.failures.len(),
        report.skipped.len()
    );

    // 2. Fetch the top hit explicitly — searching never fetches on its own.
    let Some(chosen) = report.results.first() else {
        println!("nothing to fetch");
        return Ok(());
    };
    let directory = scratch_dir("run");
    let destination = directory.join(suggest_filename(chosen));
    match engine.fetch(chosen, destination.clone()).await {
        Ok(fetched) => {
            println!(
                "saved {} ({} bytes, sha256 {})",
                fetched.path.display(),
                fetched.bytes,
                fetched.sha256.as_deref().unwrap_or("<none>")
            );
            println!("read back: {}", std::fs::read_to_string(&destination)?);
        }
        Err(error) => eprintln!("fetch failed: {error}"),
    }
    let _ = std::fs::remove_dir_all(&directory);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn searches_and_fetches_end_to_end() {
        let engine = build_engine();

        let report = engine.search(FileQuery::new("annual report")).await;
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].providers, vec![PROVIDER.to_owned()]);
        assert!(report.failures.is_empty());

        let chosen = &report.results[0];
        let directory = scratch_dir("e2e");
        let destination = directory.join(suggest_filename(chosen));
        let fetched = engine
            .fetch(chosen, destination.clone())
            .await
            .expect("fetch succeeds");

        assert_eq!(fetched.bytes, REPORT.len() as u64);
        assert_eq!(tokio::fs::read(&destination).await.unwrap(), REPORT);
        let _ = tokio::fs::remove_dir_all(&directory).await;
    }

    #[tokio::test]
    async fn fake_provider_filters_by_text() {
        let engine = build_engine();
        let report = engine.search(FileQuery::new("quarterly")).await;
        assert!(report.results.is_empty());
    }

    #[tokio::test]
    async fn fetcher_verifies_the_expected_sha256() {
        let fetcher = MemoryFetcher::new(HashMap::from([(REPORT_URI.to_owned(), REPORT.to_vec())]));
        let directory = scratch_dir("hash");
        let request = FetchRequest {
            address: Address::new("mem", REPORT_URI),
            destination: directory.join("out.bin"),
            expected_sha256: Some("deadbeef".to_owned()),
        };

        match fetcher.fetch(&request).await {
            Err(FetchError::HashMismatch { expected, actual }) => {
                assert_eq!(expected, "deadbeef");
                assert_eq!(actual, sha256_hex(REPORT));
            }
            other => panic!("expected a hash mismatch, got {other:?}"),
        }
        let _ = tokio::fs::remove_dir_all(&directory).await;
    }
}
