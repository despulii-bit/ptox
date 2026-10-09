use cwm_conformance::{check_command, check_in_process, Options};
use cwm_core::{Address, FileQuery, FileResult, ResultStream, SearchProvider};
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

struct MockProvider {
    name: String,
}

impl SearchProvider for MockProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn search(&self, _query: &FileQuery) -> ResultStream<'_> {
        let name = self.name.clone();
        Box::pin(async_stream::stream! {
            let mut result = FileResult::new(
                name,
                vec![Address::new("https", "https://example.org/x")],
            );
            result.title = Some("x".to_owned());
            yield Ok(result);
        })
    }
}

#[tokio::test]
async fn in_process_provider_passes() {
    let provider = MockProvider {
        name: "mock".to_owned(),
    };
    let report = check_in_process("mock", &provider, &Options::default()).await;
    assert!(report.passed(), "unexpected failures: {:?}", report.checks);
}
