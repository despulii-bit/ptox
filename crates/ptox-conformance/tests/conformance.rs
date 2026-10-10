use async_trait::async_trait;
use ptox::{Address, Capabilities, FileQuery, FileResult, ResultStream, SearchProvider};
use ptox_conformance::{check_command, check_in_process, Options};
use std::path::PathBuf;
use std::time::Duration;

fn fixture_bin() -> PathBuf {
    let exe = std::env::current_exe().expect("current test exe");
    let profile_dir = exe
        .parent()
        .and_then(|path| path.parent())
        .expect("target profile dir");
    profile_dir.join("examples").join("fixture_provider")
}

fn mode(name: &str) -> Vec<String> {
    vec![name.to_owned()]
}

#[tokio::test]
async fn good_subprocess_provider_passes() {
    let report = check_command(&fixture_bin(), &mode("good"), &Options::default()).await;
    assert!(report.passed(), "unexpected failures: {:?}", report.checks);
}

#[tokio::test]
async fn empty_result_set_is_still_conformant() {
    let report = check_command(&fixture_bin(), &mode("empty"), &Options::default()).await;
    assert!(report.passed(), "unexpected failures: {:?}", report.checks);
}

#[tokio::test]
async fn invalid_json_fails_the_wire_check() {
    let report = check_command(&fixture_bin(), &mode("invalid-json"), &Options::default()).await;
    assert!(!report.passed());
    assert!(report
        .checks
        .iter()
        .any(|check| check.name == "valid_wire_lines" && !check.passed));
}

#[tokio::test]
async fn bad_shape_fails_the_result_check() {
    let report = check_command(&fixture_bin(), &mode("bad-shape"), &Options::default()).await;
    assert!(!report.passed());
    assert!(report
        .checks
        .iter()
        .any(|check| check.name == "results_well_formed" && !check.passed));
}

#[tokio::test]
async fn hang_fails_the_termination_check() {
    let options = Options {
        timeout: Duration::from_millis(300),
        ..Options::default()
    };
    let report = check_command(&fixture_bin(), &mode("hang"), &options).await;
    assert!(!report.passed());
    assert!(report
        .checks
        .iter()
        .any(|check| check.name == "terminates" && !check.passed));
}

#[tokio::test]
async fn oversized_line_fails_the_wire_check() {
    let options = Options {
        max_line_bytes: 1024,
        ..Options::default()
    };
    let report = check_command(&fixture_bin(), &mode("oversized"), &options).await;
    assert!(!report.passed());
    assert!(report
        .checks
        .iter()
        .any(|check| check.name == "valid_wire_lines" && !check.passed));
}

#[tokio::test]
async fn crash_fails_the_clean_exit_check() {
    let report = check_command(&fixture_bin(), &mode("crash"), &Options::default()).await;
    assert!(!report.passed());
    assert!(report
        .checks
        .iter()
        .any(|check| check.name == "exits_cleanly" && !check.passed));
}

#[tokio::test]
async fn declared_capabilities_are_validated() {
    let report = check_command(&fixture_bin(), &mode("capabilities"), &Options::default()).await;
    assert!(report.passed(), "unexpected failures: {:?}", report.checks);
    assert!(report
        .checks
        .iter()
        .any(|check| check.name == "capabilities_well_formed" && check.passed));
}

#[tokio::test]
async fn malformed_capabilities_fail() {
    let report = check_command(
        &fixture_bin(),
        &mode("bad-capabilities"),
        &Options::default(),
    )
    .await;
    assert!(!report.passed());
    assert!(report
        .checks
        .iter()
        .any(|check| check.name == "capabilities_well_formed" && !check.passed));
}

#[tokio::test]
async fn undeclared_capabilities_are_universal() {
    let report = check_command(&fixture_bin(), &mode("good"), &Options::default()).await;
    assert!(report.passed(), "unexpected failures: {:?}", report.checks);
    assert!(report
        .checks
        .iter()
        .any(|check| check.name == "capabilities_well_formed" && check.passed));
}

struct MockProvider {
    name: String,
    capabilities: Capabilities,
    result_count: usize,
}

#[async_trait]
impl SearchProvider for MockProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn capabilities(&self) -> Capabilities {
        self.capabilities.clone()
    }

    fn search(&self, _query: &FileQuery) -> ResultStream<'_> {
        let name = self.name.clone();
        let count = self.result_count;
        Box::pin(async_stream::stream! {
            for index in 0..count {
                let mut result = FileResult::new(
                    name.clone(),
                    vec![Address::new("https", format!("https://example.org/x{index}"))],
                );
                result.title = Some(format!("x{index}"));
                yield Ok(result);
            }
        })
    }
}

#[tokio::test]
async fn in_process_provider_passes() {
    let provider = MockProvider {
        name: "mock".to_owned(),
        capabilities: Capabilities::universal(),
        result_count: 1,
    };
    let report = check_in_process("mock", &provider, &Options::default()).await;
    assert!(report.passed(), "unexpected failures: {:?}", report.checks);
}

#[tokio::test]
async fn in_process_declared_schemes_pass() {
    let provider = MockProvider {
        name: "mock".to_owned(),
        capabilities: Capabilities {
            schemes: vec!["https".to_owned()],
            ..Capabilities::default()
        },
        result_count: 1,
    };
    let report = check_in_process("mock", &provider, &Options::default()).await;
    assert!(report.passed(), "unexpected failures: {:?}", report.checks);
    assert!(report
        .checks
        .iter()
        .any(|check| check.name == "address_schemes_declared" && check.passed));
}

#[tokio::test]
async fn in_process_undeclared_schemes_fail() {
    let provider = MockProvider {
        name: "mock".to_owned(),
        capabilities: Capabilities {
            schemes: vec!["magnet".to_owned()],
            ..Capabilities::default()
        },
        result_count: 1,
    };
    let report = check_in_process("mock", &provider, &Options::default()).await;
    assert!(!report.passed());
    assert!(report
        .checks
        .iter()
        .any(|check| check.name == "address_schemes_declared" && !check.passed));
}

#[tokio::test]
async fn in_process_ignored_limit_fails() {
    let provider = MockProvider {
        name: "mock".to_owned(),
        capabilities: Capabilities::universal(),
        result_count: 3,
    };
    let report = check_in_process("mock", &provider, &Options::default()).await;
    assert!(!report.passed());
    assert!(report
        .checks
        .iter()
        .any(|check| check.name == "limit_honored" && !check.passed));
}

#[tokio::test]
async fn in_process_provider_honors_limit() {
    let provider = MockProvider {
        name: "mock".to_owned(),
        capabilities: Capabilities::universal(),
        result_count: 1,
    };
    let report = check_in_process("mock", &provider, &Options::default()).await;
    assert!(report
        .checks
        .iter()
        .any(|check| check.name == "limit_honored" && check.passed));
}

#[tokio::test]
async fn good_subprocess_provider_honors_limit() {
    let report = check_command(&fixture_bin(), &mode("good"), &Options::default()).await;
    assert!(report
        .checks
        .iter()
        .any(|check| check.name == "limit_honored" && check.passed));
}

#[tokio::test]
async fn ignored_limit_fails_the_limit_check() {
    let report = check_command(&fixture_bin(), &mode("limit-ignored"), &Options::default()).await;
    assert!(!report.passed());
    assert!(report
        .checks
        .iter()
        .any(|check| check.name == "limit_honored" && !check.passed));
}

#[tokio::test]
async fn subprocess_undeclared_schemes_fail() {
    let report = check_command(
        &fixture_bin(),
        &mode("scheme-mismatch"),
        &Options::default(),
    )
    .await;
    assert!(!report.passed());
    assert!(report
        .checks
        .iter()
        .any(|check| check.name == "address_schemes_declared" && !check.passed));
}
