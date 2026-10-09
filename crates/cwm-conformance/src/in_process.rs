use crate::{Options, Report};
use cwm_search::SearchProvider;
use cwm_types::Validate;
use futures::StreamExt;

/// Run the contract checks against an in-process provider.
pub async fn check_in_process(
    name: &str,
    provider: &dyn SearchProvider,
    options: &Options,
) -> Report {
    let mut report = Report::new(name);
    let mut results = Vec::new();
    let mut failures = Vec::new();
    let mut hung = false;

    for query in &options.queries {
        let mut stream = provider.search(query);
        let outcome = tokio::time::timeout(options.timeout, async {
            let mut items = Vec::new();
            while let Some(item) = stream.next().await {
                items.push(item);
            }
            items
        })
        .await;

        match outcome {
            Ok(items) => {
                for item in items {
                    match item {
                        Ok(result) => results.push(result),
                        Err(error) => failures.push(error),
                    }
                }
            }
            Err(_) => hung = true,
        }
    }

    report.push(
        "terminates",
        !hung,
        if hung {
            "did not finish within timeout".to_owned()
        } else {
            format!("{} result(s)", results.len())
        },
    );

    let invalid: Vec<String> = results
        .iter()
        .filter_map(|result| result.validate().err().map(|error| error.to_string()))
        .collect();
    report.push(
        "results_well_formed",
        invalid.is_empty(),
        if invalid.is_empty() {
            format!("{} result(s) valid", results.len())
        } else {
            invalid.join("; ")
        },
    );

    let bad_errors = failures
        .iter()
        .filter(|error| error.code.0.is_empty() || error.message.is_empty())
        .count();
    report.push(
        "errors_well_formed",
        bad_errors == 0,
        format!("{} error(s), {bad_errors} malformed", failures.len()),
    );

    let unattributed = results
        .iter()
        .filter(|result| result.provider.is_empty())
        .count();
    report.push(
        "results_attributed",
        unattributed == 0,
        format!("{unattributed} result(s) without a provider"),
    );

    let mismatched = results
        .iter()
        .filter(|result| result.provider != name)
        .count();
    report.push(
        "provider_name_matches",
        mismatched == 0,
        format!("{mismatched} result(s) carried a different provider name"),
    );

    report
}
