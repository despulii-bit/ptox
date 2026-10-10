use super::{temp_path, verify_sha256, FetchError, FetchRequest, Fetched, Fetcher, ProgressFn};
use async_trait::async_trait;
use futures::StreamExt;
use sha2::{Digest, Sha256};
use std::path::Path;
use tokio::io::AsyncWriteExt;

/// A descriptive default; many hosts (Wikimedia, etc.) reject requests without one.
const DEFAULT_USER_AGENT: &str = concat!(
    "ptox/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/despulii-bit/ptox)"
);

/// Fetches `http`/`https` URLs. Writes atomically and verifies sha256.
pub struct HttpFetcher {
    client: reqwest::Client,
    schemes: Vec<String>,
}

impl HttpFetcher {
    /// A fetcher for `http`/`https` using the default `User-Agent`.
    pub fn new() -> Self {
        Self::with_user_agent(DEFAULT_USER_AGENT)
    }

    /// Override the `User-Agent` sent with requests.
    pub fn with_user_agent(user_agent: &str) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(user_agent)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            client,
            schemes: vec!["http".to_owned(), "https".to_owned()],
        }
    }
}

impl Default for HttpFetcher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Fetcher for HttpFetcher {
    fn name(&self) -> &str {
        "http"
    }

    fn schemes(&self) -> &[String] {
        &self.schemes
    }

    async fn fetch(&self, request: &FetchRequest) -> Result<Fetched, FetchError> {
        self.fetch_with_progress(request, &|_: u64| {}).await
    }

    async fn fetch_with_progress(
        &self,
        request: &FetchRequest,
        progress: &ProgressFn<'_>,
    ) -> Result<Fetched, FetchError> {
        let response = self
            .client
            .get(&request.address.uri)
            .send()
            .await
            .map_err(|error| FetchError::Network(error.to_string()))?;
        if !response.status().is_success() {
            return Err(FetchError::Network(format!(
                "HTTP {} for {}",
                response.status(),
                request.address.uri
            )));
        }

        let temp = temp_path(&request.destination);
        match download(response, &temp, request, progress).await {
            Ok(fetched) => Ok(fetched),
            Err(error) => {
                // Never leave a partial download behind.
                let _ = tokio::fs::remove_file(&temp).await;
                Err(error)
            }
        }
    }
}

/// Stream `response` to `temp`, verify its hash, then rename it into place.
async fn download(
    response: reqwest::Response,
    temp: &Path,
    request: &FetchRequest,
    progress: &ProgressFn<'_>,
) -> Result<Fetched, FetchError> {
    if let Some(parent) = temp.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut file = tokio::fs::File::create(temp).await?;
    let mut hasher = Sha256::new();
    let mut stream = response.bytes_stream();
    let mut total = 0u64;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| FetchError::Network(error.to_string()))?;
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
        total += chunk.len() as u64;
        progress(total);
    }
    file.flush().await?;

    let digest = hex::encode(hasher.finalize());
    verify_sha256(request.expected_sha256.as_deref(), &digest)?;
    tokio::fs::rename(temp, &request.destination).await?;

    Ok(Fetched {
        path: request.destination.clone(),
        bytes: total,
        sha256: Some(digest),
    })
}
