use ptox::FileQuery;
use ptox_conformance::{check_command, Options, Report};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let mut timeout = Duration::from_secs(10);
    let mut max_line_bytes = ptox::MAX_LINE_BYTES;
    let mut query_texts: Vec<String> = Vec::new();
    let mut hashes: Vec<String> = Vec::new();
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
            "--hash" => {
                index += 1;
                match args.get(index) {
                    Some(pair) => hashes.push(pair.clone()),
                    None => {
                        eprintln!("error: --hash requires ALGO=VALUE");
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
    let mut hash_map = ptox::HashAlgos::new();
    for pair in &hashes {
        match pair.split_once('=') {
            Some((algo, value)) if !algo.is_empty() && !value.is_empty() => {
                hash_map.insert(algo.to_owned(), value.to_owned());
            }
            _ => {
                eprintln!("error: --hash expects ALGO=VALUE, got `{pair}`");
                return ExitCode::from(2);
            }
        }
    }

    let texts = if query_texts.is_empty() {
        if hash_map.is_empty() {
            vec!["conformance test".to_owned()]
        } else {
            vec![String::new()]
        }
    } else {
        query_texts
    };
    let queries: Vec<FileQuery> = texts
        .into_iter()
        .map(|text| {
            let mut query = FileQuery::new(text);
            if !hash_map.is_empty() {
                query.hashes = Some(hash_map.clone());
            }
            query
        })
        .collect();

    let options = Options {
        queries,
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
    "ptox-conformance — contract conformance for ptox providers

USAGE:
    ptox-conformance [OPTIONS] -- <command> [args...]

OPTIONS:
    --timeout <SECS>        per-query time budget (default 10)
    --query <TEXT>          query to send (repeatable)
    --hash <ALGO=VALUE>     hash selector, e.g. `cid=bafy...` (repeatable)
    --max-line-bytes <N>    reject provider lines larger than N bytes
    --json                  emit the report as JSON
    -h, --help              show this help
"
}
