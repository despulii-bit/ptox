use crate::{sha256_file, verify_sha256, FetchError, FetchRequest, Fetched, Fetcher};
use async_trait::async_trait;

/// Delegates a scheme to an external command (e.g. `aria2c` for `magnet`).
///
/// Each argument may contain `{uri}` (the address URI) and `{dest}` (the
/// destination path). The command is expected to create the destination file.
pub struct CommandFetcher {
    name: String,
    schemes: &'static [&'static str],
    program: String,
    args: Vec<String>,
}

impl CommandFetcher {
    pub fn new(
        name: impl Into<String>,
        schemes: &'static [&'static str],
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
}

#[async_trait]
impl Fetcher for CommandFetcher {
    fn name(&self) -> &str {
        &self.name
    }

    fn schemes(&self) -> &'static [&'static str] {
        self.schemes
    }

    async fn fetch(&self, request: &FetchRequest) -> Result<Fetched, FetchError> {
        if let Some(parent) = request.destination.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let destination = request.destination.to_string_lossy().into_owned();
        let args: Vec<String> = self
            .args
            .iter()
            .map(|arg| {
                arg.replace("{uri}", &request.address.uri)
                    .replace("{dest}", &destination)
            })
            .collect();

        let status = tokio::process::Command::new(&self.program)
            .args(&args)
            .status()
            .await
            .map_err(|error| FetchError::Command(format!("{}: {error}", self.program)))?;
        if !status.success() {
            return Err(FetchError::Command(format!(
                "{} exited with {status}",
                self.program
            )));
        }

        let bytes = tokio::fs::metadata(&request.destination)
            .await
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let digest = sha256_file(&request.destination).await.ok();
        if let Some(actual) = digest.as_deref() {
            verify_sha256(request.expected_sha256.as_deref(), actual)?;
        }

        Ok(Fetched {
            path: request.destination.clone(),
            bytes,
            sha256: digest,
        })
    }
}
