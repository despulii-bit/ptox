use cwm::FileQuery;
use cwm_conformance::{check_command, Options, Report};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let mut timeout = Duration::from_secs(10);
    let mut max_line_bytes = cwm::MAX_LINE_BYTES;
    let mut query_texts: Vec<String> = Vec::new();
    let mut json = false;
    let mut command: Vec<String> = Vec::new();

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--" => {
                command = args[index + 1..].to_vec();
                break;
            }
            "--timeout" => {
                index += 1;
                match args.get(index).and_then(|value| value.parse::<u64>().ok()) {
                    Some(seconds) => timeout = Duration::from_secs(seconds),
                    None => {
                        eprintln!("error: --timeout requires a number of seconds");
                        return ExitCode::from(2);
                    }
                }
            }
            "--query" => {
                index += 1;
                match args.get(index) {
                    Some(text) => query_texts.push(text.clone()),
                    None => {
                        eprintln!("error: --query requires text");
                        return ExitCode::from(2);
                    }
                }
            }
            "--json" => json = true,
            "--max-line-bytes" => {
                index += 1;
                match args
                    .get(index)
                    .and_then(|value| value.parse::<usize>().ok())
                {
                    Some(bytes) => max_line_bytes = bytes,
                    None => {
                        eprintln!("error: --max-line-bytes requires a byte count");
                        return ExitCode::from(2);
                    }
                }
            }
            "-h" | "--help" => {
                print!("{}", usage());
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("error: unknown option `{other}`\n{}", usage());
                return ExitCode::from(2);
            }
        }
        index += 1;
    }

    if command.is_empty() {
        eprintln!(
            "error: missing provider command (put it after `--`)\n{}",
            usage()
        );
        return ExitCode::from(2);
    }

    let program = PathBuf::from(&command[0]);
    let program_args = command[1..].to_vec();
    let options = Options {
        queries: if query_texts.is_empty() {
            vec![FileQuery::new("conformance test")]
        } else {
            query_texts.into_iter().map(FileQuery::new).collect()
        },
        timeout,
        max_line_bytes,
    };

    let report = check_command(&program, &program_args, &options).await;
    print_report(&report, json);

    if report.passed() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn print_report(report: &Report, json: bool) {
    if json {
        match serde_json::to_string_pretty(report) {
            Ok(text) => println!("{text}"),
            Err(error) => eprintln!("error: {error}"),
        }
        return;
    }

    println!("provider: {}", report.provider);
    for check in &report.checks {
        let status = if check.passed { "PASS" } else { "FAIL" };
        println!("  {status} {}: {}", check.name, check.detail);
    }
    let failed = report.failures().count();
    println!("{} passed, {failed} failed", report.checks.len() - failed);
}

fn usage() -> &'static str {
    "cwm-conformance — contract conformance for cwm providers

USAGE:
    cwm-conformance [OPTIONS] -- <command> [args...]

OPTIONS:
    --timeout <SECS>        per-query time budget (default 10)
    --query <TEXT>          query to send (repeatable)
    --max-line-bytes <N>    reject provider lines larger than N bytes
    --json                  emit the report as JSON
    -h, --help              show this help
"
}
