use crate::{Options, Report};
use cwm_core::{encode_request, parse_line, ErrorInfo, FileQuery, FileResult, Validate, WireLine};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::Instant;

/// Run the contract checks against a provider subprocess speaking NDJSON.
///
/// One fresh process is spawned per query, matching the current wire protocol.
pub async fn check_command(program: &Path, args: &[String], options: &Options) -> Report {
    let name = program.display().to_string();
    let mut report = Report::new(&name);

    let mut spawn_errors = Vec::new();
    let mut invalid = Vec::new();
    let mut results = Vec::new();
    let mut errors = Vec::new();
    let mut not_terminated = 0usize;

    for query in &options.queries {
        match run_once(program, args, query, options.timeout).await {
            Ok(run) => {
                invalid.extend(run.invalid);
                for mut result in run.results {
                    // The host owns attribution (contract §9).
                    if result.provider.is_empty() {
                        result.provider = name.clone();
                    }
                    results.push(result);
                }
                errors.extend(run.errors);
                if !run.terminated {
                    not_terminated += 1;
                }
            }
            Err(error) => spawn_errors.push(error),
        }
    }

    report.push(
        "spawns",
        spawn_errors.is_empty(),
        if spawn_errors.is_empty() {
            format!("{} invocation(s)", options.queries.len())
        } else {
            spawn_errors.join("; ")
        },
    );
    report.push(
        "valid_wire_lines",
        invalid.is_empty(),
        if invalid.is_empty() {
            "all output lines parsed".to_owned()
        } else {
            invalid.join("; ")
        },
    );

    let invalid_results: Vec<String> = results
        .iter()
        .filter_map(|result| result.validate().err().map(|error| error.to_string()))
        .collect();
    report.push(
        "results_well_formed",
        invalid_results.is_empty(),
        if invalid_results.is_empty() {
            format!("{} result(s) valid", results.len())
        } else {
            invalid_results.join("; ")
        },
    );

    let bad_errors = errors
        .iter()
        .filter(|error| error.code.0.is_empty() || error.message.is_empty())
        .count();
    report.push(
        "errors_well_formed",
        bad_errors == 0,
        format!("{} error(s), {bad_errors} malformed", errors.len()),
    );

    report.push(
        "terminates",
        not_terminated == 0,
        format!("{not_terminated} invocation(s) did not finish"),
    );

    report
}

#[derive(Default)]
struct Run {
    results: Vec<FileResult>,
    errors: Vec<ErrorInfo>,
    invalid: Vec<String>,
    terminated: bool,
}

async fn run_once(
    program: &Path,
    args: &[String],
    query: &FileQuery,
    timeout: Duration,
) -> Result<Run, String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    let mut child = command.spawn().map_err(|error| error.to_string())?;

    let request = encode_request(1, query).map_err(|error| error.to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(request.as_bytes())
            .await
            .map_err(|error| error.to_string())?;
        // Dropping stdin signals EOF.
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "provider has no stdout".to_owned())?;
    let mut lines = BufReader::new(stdout).lines();
    let mut run = Run::default();
    let deadline = Instant::now() + timeout;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, lines.next_line()).await {
            Err(_) => break,
            Ok(Ok(Some(line))) => {
                if line.trim().is_empty() {
                    continue;
                }
                match parse_line(&line) {
                    WireLine::Result(result) => run.results.push(result),
                    WireLine::Error(error) => run.errors.push(error),
                    WireLine::Done => {
                        run.terminated = true;
                        break;
                    }
                    WireLine::Invalid { message } => run.invalid.push(message),
                }
            }
            Ok(Ok(None)) => {
                run.terminated = true;
                break;
            }
            Ok(Err(error)) => {
                run.invalid.push(format!("read error: {error}"));
                break;
            }
        }
    }

    let _ = child.kill().await;
    let _ = child.wait().await;

    Ok(run)
}
