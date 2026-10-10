use crate::{Options, Report};
use ptox::{
    encode_capabilities_request, encode_request, parse_line, read_line_bounded, Capabilities,
    ErrorInfo, FileQuery, FileResult, Validate, WireLine, WireReadError,
};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncWriteExt, BufReader};
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
    let mut abnormal = Vec::new();
    let mut not_terminated = 0usize;

    for query in &options.queries {
        match run_once(
            program,
            args,
            query,
            options.timeout,
            options.max_line_bytes,
        )
        .await
        {
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
                if let Some(reason) = run.abnormal_exit {
                    abnormal.push(format!("{name}: {reason}"));
                }
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

    report.push(
        "exits_cleanly",
        abnormal.is_empty(),
        if abnormal.is_empty() {
            "no abnormal exits".to_owned()
        } else {
            abnormal.join("; ")
        },
    );

    // Capability negotiation is optional: a provider that declines is treated as
    // universal. One that *does* answer must return a well-formed object.
    let capabilities_timeout = options.timeout.min(Duration::from_secs(2));
    match probe_capabilities(program, args, capabilities_timeout, options.max_line_bytes).await {
        CapabilityReport::NotDeclared => report.push(
            "capabilities_well_formed",
            true,
            "not declared (treated as universal)",
        ),
        CapabilityReport::Declared(capabilities) => report.push(
            "capabilities_well_formed",
            true,
            format!(
                "declared: query={:?} requires={:?} schemes={:?}",
                capabilities.query, capabilities.requires, capabilities.schemes
            ),
        ),
        CapabilityReport::Malformed(message) => {
            report.push("capabilities_well_formed", false, message)
        }
    }

    report
}

#[derive(Default)]
struct Run {
    results: Vec<FileResult>,
    errors: Vec<ErrorInfo>,
    invalid: Vec<String>,
    terminated: bool,
    abnormal_exit: Option<String>,
}

/// What a subprocess provider said when asked for its capabilities.
enum CapabilityReport {
    /// Declined to answer (done, error, timeout, or no response): universal.
    NotDeclared,
    /// Answered with a parseable capability object.
    Declared(Capabilities),
    /// Answered, but the answer violated the wire contract.
    Malformed(String),
}

/// Ask a provider for its capabilities once. Never fails the provider for not
/// answering — capabilities are optional.
async fn probe_capabilities(
    program: &Path,
    args: &[String],
    timeout: Duration,
    max_line_bytes: usize,
) -> CapabilityReport {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    let Ok(mut child) = command.spawn() else {
        return CapabilityReport::NotDeclared;
    };
    let Ok(request) = encode_capabilities_request(1) else {
        return CapabilityReport::NotDeclared;
    };
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(request.as_bytes()).await.is_err() {
            return CapabilityReport::NotDeclared;
        }
    }
    let Some(stdout) = child.stdout.take() else {
        return CapabilityReport::NotDeclared;
    };

    let mut reader = BufReader::new(stdout);
    let deadline = Instant::now() + timeout;
    let mut report = CapabilityReport::NotDeclared;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, read_line_bounded(&mut reader, max_line_bytes)).await
        {
            Ok(Ok(Some(line))) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let value: serde_json::Value = match serde_json::from_str(line) {
                    Ok(value) => value,
                    Err(error) => {
                        report = CapabilityReport::Malformed(format!("invalid json: {error}"));
                        break;
                    }
                };
                if value.get("done").and_then(serde_json::Value::as_bool) == Some(true)
                    || value.get("error").is_some()
                {
                    break;
                }
                if let Some(capabilities) = value.get("result") {
                    report = match serde_json::from_value::<Capabilities>(capabilities.clone()) {
                        Ok(capabilities) => CapabilityReport::Declared(capabilities),
                        Err(error) => CapabilityReport::Malformed(format!(
                            "invalid capabilities object: {error}"
                        )),
                    };
                    break;
                }
                report = CapabilityReport::Malformed(
                    "line has neither `result`, `error`, nor `done`".to_owned(),
                );
                break;
            }
            Ok(Ok(None)) => break,
            Ok(Err(WireReadError::TooLong { limit })) => {
                report = CapabilityReport::Malformed(format!("line exceeded {limit} bytes"));
                break;
            }
            Ok(Err(WireReadError::Io(error))) => {
                report = CapabilityReport::Malformed(format!("read error: {error}"));
                break;
            }
            Err(_) => break,
        }
    }

    let _ = child.kill().await;
    let _ = child.wait().await;
    report
}

async fn run_once(
    program: &Path,
    args: &[String],
    query: &FileQuery,
    timeout: Duration,
    max_line_bytes: usize,
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
    let mut reader = BufReader::new(stdout);
    let mut run = Run::default();
    let mut saw_eof = false;
    let deadline = Instant::now() + timeout;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, read_line_bounded(&mut reader, max_line_bytes)).await
        {
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
                saw_eof = true;
                break;
            }
            Ok(Err(WireReadError::TooLong { limit })) => {
                run.invalid.push(format!("line exceeded {limit} bytes"));
                break;
            }
            Ok(Err(WireReadError::Io(error))) => {
                run.invalid.push(format!("read error: {error}"));
                break;
            }
        }
    }

    if saw_eof {
        if let Ok(Ok(status)) = tokio::time::timeout(Duration::from_secs(1), child.wait()).await {
            if !status.success() {
                run.abnormal_exit = Some(format!("exited with {status}"));
            }
        }
    }

    let _ = child.kill().await;
    let _ = child.wait().await;

    Ok(run)
}
