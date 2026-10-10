//! A [`SearchProvider`] backed by an external process speaking the NDJSON wire
//! format. This is how providers written in other languages plug in.

use super::wire::{
    encode_capabilities_request, encode_request, parse_line, read_line_bounded, WireLine,
    WireReadError, MAX_LINE_BYTES,
};
use crate::search::{Capabilities, ProviderError, ResultStream, SearchProvider};
use crate::types::{ErrorCode, FileQuery};
use async_stream::stream;
use async_trait::async_trait;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::{timeout, Instant};

/// Runs a child process once per query. The child is killed when the stream is
/// dropped, so cancellation propagates across the process boundary.
///
/// Configuration (proxies, routers, data dirs, credentials) is passed to the
/// provider at spawn time via arguments and environment variables.
#[derive(Debug, Clone)]
pub struct SubprocessProvider {
    name: String,
    program: PathBuf,
    args: Vec<String>,
    env: Vec<(String, String)>,
    capabilities: Arc<Mutex<Option<Capabilities>>>,
}

impl SubprocessProvider {
    /// A provider named `name` that runs `program` once per query.
    pub fn new(name: impl Into<String>, program: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
            capabilities: Arc::new(Mutex::new(None)),
        }
    }

    /// Append one command-line argument.
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Append several command-line arguments.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Set one environment variable for the child process.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Set several environment variables for the child process.
    pub fn envs<I, K, V>(mut self, vars: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.env.extend(
            vars.into_iter()
                .map(|(key, value)| (key.into(), value.into())),
        );
        self
    }

    /// The cached capabilities, if a probe has already completed successfully.
    fn cached_capabilities(&self) -> Option<Capabilities> {
        self.capabilities.lock().ok().and_then(|slot| slot.clone())
    }

    /// Ask the provider for its capabilities once; `None` if it doesn't answer.
    async fn probe_capabilities(&self) -> Option<Capabilities> {
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .envs(
                self.env
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command.spawn().ok()?;

        let request = encode_capabilities_request(1).ok()?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(request.as_bytes()).await.ok()?;
        }
        let stdout = child.stdout.take()?;
        let mut reader = BufReader::new(stdout);
        let deadline = Instant::now() + Duration::from_secs(2);

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            match timeout(remaining, read_line_bounded(&mut reader, MAX_LINE_BYTES)).await {
                Ok(Ok(Some(line))) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let value: serde_json::Value = match serde_json::from_str(&line) {
                        Ok(value) => value,
                        Err(_) => return None,
                    };
                    if value.get("done").and_then(serde_json::Value::as_bool) == Some(true) {
                        return None;
                    }
                    if value.get("error").is_some() {
                        return None;
                    }
                    if let Some(result) = value.get("result") {
                        return serde_json::from_value::<Capabilities>(result.clone()).ok();
                    }
                }
                _ => return None,
            }
        }
    }
}

#[async_trait]
impl SearchProvider for SubprocessProvider {
    fn name(&self) -> &str {
        &self.name
    }

    /// Ask the provider once, then cache; unknown is treated as universal.
    async fn capabilities(&self) -> Capabilities {
        if let Some(capabilities) = self.cached_capabilities() {
            return capabilities;
        }
        let capabilities = self
            .probe_capabilities()
            .await
            .unwrap_or_else(Capabilities::universal);
        if let Ok(mut slot) = self.capabilities.lock() {
            *slot = Some(capabilities.clone());
        }
        capabilities
    }

    fn invalidate_capabilities(&self) {
        if let Ok(mut slot) = self.capabilities.lock() {
            *slot = None;
        }
    }

    fn search(&self, query: &FileQuery) -> ResultStream<'_> {
        let name = self.name.clone();
        let program = self.program.clone();
        let args = self.args.clone();
        let env = self.env.clone();
        let query = query.clone();

        Box::pin(stream! {
            let mut command = Command::new(&program);
            command
                .args(&args)
                .envs(env.iter().map(|(key, value)| (key.clone(), value.clone())))
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

            let mut reader = BufReader::new(stdout);
            loop {
                match read_line_bounded(&mut reader, MAX_LINE_BYTES).await {
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
                    Ok(None) => {
                        // EOF without `done`. If the process exited non-zero it
                        // crashed — report a failure, not an empty result set.
                        if let Ok(Ok(status)) =
                            tokio::time::timeout(Duration::from_secs(1), child.wait()).await
                        {
                            if !status.success() {
                                yield Err(ProviderError::new(
                                    ErrorCode::transport(),
                                    format!("provider exited with {status}"),
                                ));
                            }
                        }
                        return;
                    }
                    Err(WireReadError::TooLong { limit }) => {
                        yield Err(ProviderError::new(
                            ErrorCode::malformed(),
                            format!("provider output line exceeded {limit} bytes"),
                        ));
                        return;
                    }
                    Err(WireReadError::Io(error)) => {
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
