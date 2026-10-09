//! A [`SearchProvider`] backed by an external process speaking the NDJSON wire
//! format. This is how providers written in other languages plug in.

use crate::provider::{ProviderError, ResultStream, SearchProvider};
use crate::types::{ErrorCode, FileQuery};
use crate::wire::{encode_request, parse_line, WireLine};
use async_stream::stream;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

/// Runs a child process once per query. The child is killed when the stream is
/// dropped, so cancellation propagates across the process boundary.
#[derive(Debug, Clone)]
pub struct SubprocessProvider {
    name: String,
    program: PathBuf,
    args: Vec<String>,
}

impl SubprocessProvider {
    pub fn new(name: impl Into<String>, program: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            program: program.into(),
            args: Vec::new(),
        }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }
}

impl SearchProvider for SubprocessProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn search(&self, query: &FileQuery) -> ResultStream<'_> {
        let name = self.name.clone();
        let program = self.program.clone();
        let args = self.args.clone();
        let query = query.clone();

        Box::pin(stream! {
            let mut command = Command::new(&program);
            command
                .args(&args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true);

            let mut child = match command.spawn() {
                Ok(child) => child,
                Err(error) => {
                    yield Err(ProviderError::new(
                        ErrorCode::transport(),
                        format!("failed to spawn provider: {error}"),
                    ));
                    return;
                }
            };

            let request = match encode_request(1, &query) {
                Ok(request) => request,
                Err(error) => {
                    yield Err(ProviderError::new(
                        ErrorCode::internal(),
                        format!("failed to encode request: {error}"),
                    ));
                    return;
                }
            };

            if let Some(mut stdin) = child.stdin.take() {
                if let Err(error) = stdin.write_all(request.as_bytes()).await {
                    yield Err(ProviderError::new(
                        ErrorCode::transport(),
                        format!("failed to write request: {error}"),
                    ));
                    return;
                }
                // Dropping stdin signals EOF.
            }

            let Some(stdout) = child.stdout.take() else {
                yield Err(ProviderError::new(
                    ErrorCode::transport(),
                    "provider has no stdout",
                ));
                return;
            };

            let mut lines = BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        match parse_line(&line) {
                            WireLine::Result(mut result) => {
                                // Attribution is never left to the provider.
                                result.provider = name.clone();
                                yield Ok(result);
                            }
                            WireLine::Error(error) => {
                                yield Err(ProviderError {
                                    code: error.code,
                                    message: error.message,
                                });
                                return;
                            }
                            WireLine::Done => return,
                            WireLine::Invalid { message } => {
                                yield Err(ProviderError::new(ErrorCode::malformed(), message));
                                return;
                            }
                        }
                    }
                    Ok(None) => return,
                    Err(error) => {
                        yield Err(ProviderError::new(
                            ErrorCode::transport(),
                            format!("failed to read provider output: {error}"),
                        ));
                        return;
                    }
                }
            }
        })
    }
}
