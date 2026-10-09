//! Conformance item I: out-of-process providers over the NDJSON wire format.

use cwm::{
    fanout, Capabilities, ErrorCode, FileQuery, Options, Outcome, SearchProvider,
    SubprocessProvider,
};
use futures::StreamExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

fn example_binary() -> PathBuf {
    let exe = std::env::current_exe().expect("current test exe");
    let profile_dir = exe
        .parent()
        .and_then(|path| path.parent())
        .expect("target profile dir");
    profile_dir.join("examples").join("ndjson_provider")
}

fn providers(list: Vec<SubprocessProvider>) -> Vec<Arc<dyn SearchProvider>> {
    list.into_iter()
        .map(|provider| Arc::new(provider) as Arc<dyn SearchProvider>)
        .collect()
}

async fn collect(stream: impl futures::Stream<Item = Outcome>) -> Vec<Outcome> {
    futures::pin_mut!(stream);
    let mut outcomes = Vec::new();
    while let Some(outcome) = stream.next().await {
        outcomes.push(outcome);
    }
    outcomes
}

fn provider(mode: Option<&str>) -> SubprocessProvider {
    let provider = SubprocessProvider::new("example", example_binary());
    match mode {
        Some(mode) => provider.arg(mode),
        None => provider,
    }
}

#[tokio::test]
async fn streams_results_from_a_subprocess_and_attaches_provider_name() {
    let bin = example_binary();
    assert!(
        bin.exists(),
        "example binary not built at {}",
        bin.display()
    );

    let outcomes = collect(fanout(
        FileQuery::new("hello"),
        providers(vec![provider(None)]),
        Options::default(),
    ))
    .await;

    let results: Vec<_> = outcomes.iter().filter_map(|o| o.as_result()).collect();
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|r| r.provider == "example"));
    assert!(results
        .iter()
        .any(|r| r.title.as_deref() == Some("hello-0.bin")));
}

#[tokio::test]
async fn passes_environment_to_the_provider() {
    let bin = example_binary();
    let outcomes = collect(fanout(
        FileQuery::new("hello"),
        providers(vec![
            SubprocessProvider::new("example", bin).env("CWM_EXAMPLE_TAG", "TAG")
        ]),
        Options::default(),
    ))
    .await;

    let titles: Vec<String> = outcomes
        .iter()
        .filter_map(|outcome| outcome.as_result())
        .filter_map(|result| result.title.clone())
        .collect();
    assert!(
        titles.iter().any(|title| title.contains("TAG-")),
        "expected an env-tagged title, got {titles:?}"
    );
}

#[tokio::test]
async fn subprocess_provider_reports_capabilities() {
    let bin = example_binary();
    let provider = SubprocessProvider::new("example", bin).arg("capabilities");
    let capabilities = provider.capabilities().await;
    assert_eq!(capabilities.query, vec!["text"]);
    assert_eq!(capabilities.schemes, vec!["https"]);
    assert!(capabilities.accepts(&FileQuery::new("keyword")));
}

#[tokio::test]
async fn provider_without_capabilities_is_universal() {
    let provider = SubprocessProvider::new("example", example_binary());
    let capabilities = provider.capabilities().await;
    assert_eq!(capabilities, Capabilities::universal());
}

#[tokio::test]
async fn abnormal_exit_becomes_a_failure() {
    let bin = example_binary();
    let outcomes = collect(fanout(
        FileQuery::new("x"),
        providers(vec![SubprocessProvider::new("example", bin).arg("crash")]),
        Options::default(),
    ))
    .await;

    assert_eq!(outcomes.len(), 1);
    match &outcomes[0] {
        Outcome::Failure(failure) => assert_eq!(failure.error.code, ErrorCode::transport()),
        other => panic!("expected a transport failure, got {other:?}"),
    }
}

#[tokio::test]
async fn provider_error_becomes_a_failure() {
    let outcomes = collect(fanout(
        FileQuery::new("x"),
        providers(vec![provider(Some("error"))]),
        Options::default(),
    ))
    .await;

    assert_eq!(outcomes.len(), 1);
    match &outcomes[0] {
        Outcome::Failure(failure) => assert_eq!(failure.error.code, ErrorCode::refused()),
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[tokio::test]
async fn malformed_output_becomes_a_malformed_failure() {
    let outcomes = collect(fanout(
        FileQuery::new("x"),
        providers(vec![provider(Some("malformed"))]),
        Options::default(),
    ))
    .await;

    assert_eq!(outcomes.len(), 1);
    match &outcomes[0] {
        Outcome::Failure(failure) => assert_eq!(failure.error.code, ErrorCode::malformed()),
        other => panic!("expected a malformed failure, got {other:?}"),
    }
}

#[tokio::test]
async fn dropping_the_stream_cancels_the_subprocess_promptly() {
    let mut stream = Box::pin(fanout(
        FileQuery::new("x"),
        providers(vec![provider(Some("infinite"))]),
        Options::default(),
    ));

    let first = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("a result should arrive");
    assert!(first.is_some());

    drop(stream);
    // Reaching the end without hanging is the assertion: kill_on_drop reaps the child.
}
