use super::{sha256_file, verify_sha256, FetchError, FetchRequest, Fetched, Fetcher, ProgressFn};
use async_trait::async_trait;
use std::time::Duration;

/// How often the destination file size is checked while a delegated command
/// runs. The command writes to the destination in place, so its size is the
/// only progress signal available.
const PROGRESS_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Delegates a scheme to an external command (e.g. `aria2c` for `magnet`).
///
/// Each argument may contain `{uri}` (the address URI), `{dest}` (the full
/// destination path), `{dir}` (its parent directory), and `{name}` (its file
/// name). The command is expected to create the destination file.
///
/// The command writes the destination in place, so ptox cannot observe chunks
/// as they arrive the way [`super::HttpFetcher`] can. Instead,
/// [`Fetcher::fetch_with_progress`] polls the destination file size while the
/// command runs and reports each increase, so clients can still animate a
/// progress bar for command-delegated schemes such as magnets.
pub struct CommandFetcher {
    name: String,
    schemes: Vec<String>,
    program: String,
    args: Vec<String>,
}

impl CommandFetcher {
    /// A fetcher named `name` that runs `program` with `args` for each of `schemes`.
    pub fn new(
        name: impl Into<String>,
        schemes: Vec<String>,
        program: impl Into<String>,
        args: Vec<String>,
    ) -> Self {
        Self {
            name: name.into(),
            schemes,
            program: program.into(),
            args,
        }
    }

    /// Substitute the `{uri}`/`{dest}`/`{dir}`/`{name}` placeholders for `request`.
    fn build_args(&self, request: &FetchRequest) -> Vec<String> {
        let destination = request.destination.to_string_lossy().into_owned();
        let directory = request
            .destination
            .parent()
            .map(|parent| parent.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = request
            .destination
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();

        self.args
            .iter()
            .map(|arg| {
                arg.replace("{uri}", &request.address.uri)
                    .replace("{dest}", &destination)
                    .replace("{dir}", &directory)
                    .replace("{name}", &name)
            })
            .collect()
    }

    /// Create the destination's parent directory, run the command, and fail if
    /// it exits non-zero.
    ///
    /// When `progress` is `Some`, the destination size is polled while the
    /// command runs and every increase is reported. The file is checked once
    /// more after the command exits, so the final report always reflects the
    /// complete download even if the command finished between two polls.
    async fn run(
        &self,
        request: &FetchRequest,
        progress: Option<&ProgressFn<'_>>,
    ) -> Result<(), FetchError> {
        if let Some(parent) = request.destination.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let args = self.build_args(request);
        let mut child = tokio::process::Command::new(&self.program)
            .args(&args)
            .spawn()
            .map_err(|error| FetchError::Command(format!("{}: {error}", self.program)))?;

        let status = match progress {
            None => child
                .wait()
                .await
                .map_err(|error| FetchError::Command(format!("{}: {error}", self.program)))?,
            Some(progress) => {
                let mut last = 0u64;
                loop {
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            report_size(&request.destination, progress, &mut last).await;
                            break status;
                        }
                        Ok(None) => {}
                        Err(error) => {
                            return Err(FetchError::Command(format!("{}: {error}", self.program)));
                        }
                    }
                    report_size(&request.destination, progress, &mut last).await;
                    tokio::time::sleep(PROGRESS_POLL_INTERVAL).await;
                }
            }
        };

        if !status.success() {
            return Err(FetchError::Command(format!(
                "{} exited with {status}",
                self.program
            )));
        }
        Ok(())
    }

    /// Measure the finished destination, verify its hash, and return the outcome.
    async fn finish(&self, request: &FetchRequest) -> Result<Fetched, FetchError> {
        // A command that exits 0 without producing the destination is a failure,
        // not a zero-byte success.
        let bytes = tokio::fs::metadata(&request.destination)
            .await
            .map_err(|error| {
                FetchError::Command(format!(
                    "{} exited successfully but did not create {}: {error}",
                    self.program,
                    request.destination.display()
                ))
            })?
            .len();
        let digest = sha256_file(&request.destination).await.ok();
        if let Some(actual) = digest.as_deref() {
            verify_sha256(request.expected_sha256.as_deref(), actual)?;
        }

        Ok(Fetched {
            path: request.destination.clone(),
            bytes,
            sha256: digest,
            address: request.address.clone(),
        })
    }
}

/// Report the destination size to `progress` if it grew since `last`.
async fn report_size(destination: &std::path::Path, progress: &ProgressFn<'_>, last: &mut u64) {
    if let Ok(metadata) = tokio::fs::metadata(destination).await {
        let bytes = metadata.len();
        if bytes > *last {
            *last = bytes;
            progress(bytes);
        }
    }
}

#[async_trait]
impl Fetcher for CommandFetcher {
    fn name(&self) -> &str {
        &self.name
    }

    fn schemes(&self) -> &[String] {
        &self.schemes
    }

    async fn fetch(&self, request: &FetchRequest) -> Result<Fetched, FetchError> {
        self.run(request, None).await?;
        self.finish(request).await
    }

    async fn fetch_with_progress(
        &self,
        request: &FetchRequest,
        progress: &ProgressFn<'_>,
    ) -> Result<Fetched, FetchError> {
        self.run(request, Some(progress)).await?;
        self.finish(request).await
    }
}
