use super::{sha256_file, verify_sha256, FetchError, FetchRequest, Fetched, Fetcher};
use async_trait::async_trait;

/// Delegates a scheme to an external command (e.g. `aria2c` for `magnet`).
///
/// Each argument may contain `{uri}` (the address URI), `{dest}` (the full
/// destination path), `{dir}` (its parent directory), and `{name}` (its file
/// name). The command is expected to create the destination file.
pub struct CommandFetcher {
    name: String,
    schemes: Vec<String>,
    program: String,
    args: Vec<String>,
}

impl CommandFetcher {
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
        if let Some(parent) = request.destination.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

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

        let args: Vec<String> = self
            .args
            .iter()
            .map(|arg| {
                arg.replace("{uri}", &request.address.uri)
                    .replace("{dest}", &destination)
                    .replace("{dir}", &directory)
                    .replace("{name}", &name)
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
