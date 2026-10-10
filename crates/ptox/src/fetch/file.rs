//! Retrieve `file://` addresses from the local filesystem.
//!
//! Some providers are **local**: a catalogue scanner, an on-disk index, or a
//! mock used in tests emits `Address`es that point at a file already on the
//! machine. Without a handler the scheme registry rejects them with
//! [`FetchError::Unsupported`], so an offline provider cannot be fetched at all.
//! This module bridges that with a [`FileFetcher`] for the `file` scheme.
//!
//! It upholds the same retrieval guarantees as [`super::HttpFetcher`]: the bytes
//! are copied to `<destination>.part`, hashed while streaming (so a large file
//! never has to fit in memory), verified against `expected_sha256`, and only
//! then atomically renamed into place. Progress is reported per chunk.
//!
//! The `file` feature is **off by default**; enable it in `Cargo.toml`:
//!
//! ```toml
//! ptox = { version = "0.1", features = ["file"] }
//! ```
//!
//! ```no_run
//! # async fn run() -> Result<(), ptox::FetchError> {
//! use ptox::{Address, FetchRegistry, FileFetcher};
//! use std::sync::Arc;
//!
//! let mut registry = FetchRegistry::new();
//! registry.register(Arc::new(FileFetcher::new()));
//! let address = Address::new("file", "file:///data/reports/annual.pdf");
//! let fetched = registry.fetch(&address, "annual.pdf", None).await?;
//! println!("{} bytes -> {}", fetched.bytes, fetched.path.display());
//! # Ok(())
//! # }
//! ```
//!
//! # Local-file policy
//!
//! A `file://` address is a **read capability for the whole filesystem**. A
//! provider that emits `file:///etc/passwd` (or any other absolute path) makes
//! the client copy that file into its download directory, and a client that
//! fetches a merged result automatically will do so without asking. This is why
//! the fetcher is behind an opt-in feature rather than on by default, and why
//! enabling it should be a deliberate decision:
//!
//! - Enable `file` only when you run **local, trusted providers** (or when the
//!   machine is a scratch/lab environment). Treat any remote provider as hostile
//!   if `file` is enabled.
//! - Prefer [`FileFetcher::with_allowed_roots`], which refuses any source that
//!   resolves (after symlinks) outside the directories you name. A client that
//!   has a data directory should pass it so a provider cannot escape it.
//! - The default `FileFetcher::new()` allows any path the process can read,
//!   matching the convenience of `cp`; use it only in that spirit.
//!
//! `with_defaults` / `Config::fetch_registry` register the fetcher when the
//! feature is enabled, so an explicitly enabled build works without further
//! wiring.
//!
//! # URI parsing
//!
//! `file://<path>` URIs are parsed as follows:
//!
//! - `file:///abs/path` → `/abs/path` (empty authority);
//! - `file://localhost/abs/path` → `/abs/path` (a `localhost` authority is
//!   accepted, case-insensitively);
//! - any other authority (`file://host/share`) is rejected with
//!   [`FetchError::InvalidAddress`], so a URI cannot silently mean a remote
//!   share;
//! - `%XX` escapes are decoded, so `file:///tmp/a%20b.txt` means
//!   `/tmp/a b.txt`; an invalid escape is rejected.
//!
//! The scheme is a client-side choice: ptox neither emits nor validates
//! `file://` results, so a provider can still be probed for a scheme the client
//! chooses not to resolve.

use super::{temp_path, verify_sha256, FetchError, FetchRequest, Fetched, Fetcher, ProgressFn};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The scheme this fetcher resolves.
const FILE_SCHEME: &str = "file";

/// How many bytes are read (and written) per progress report.
const READ_CHUNK: usize = 64 * 1024;

/// Resolves `file://` addresses by copying a local file to the destination.
///
/// Writes atomically, verifies sha256, and reports cumulative progress per
/// chunk. See `docs/fetching.md` for the local-file policy.
pub struct FileFetcher {
    schemes: Vec<String>,
    allowed_roots: Option<Vec<PathBuf>>,
}

impl FileFetcher {
    /// A fetcher that may read **any** path the process can read.
    ///
    /// Convenient for local/scratch providers; see `docs/fetching.md` and
    /// prefer [`FileFetcher::with_allowed_roots`] when a data directory is known.
    pub fn new() -> Self {
        Self {
            schemes: vec![FILE_SCHEME.to_owned()],
            allowed_roots: None,
        }
    }

    /// A fetcher restricted to `roots`: a source is fetched only if, after
    /// resolving symlinks, it lives inside one of them.
    ///
    /// A root that does not exist is ignored (it simply matches nothing). This is
    /// the safe choice when a provider's `file://` addresses should stay within a
    /// known data directory. A path outside every root fails with
    /// [`FetchError::AccessDenied`].
    ///
    /// ```
    /// use ptox::FileFetcher;
    /// use std::path::PathBuf;
    ///
    /// let fetcher = FileFetcher::with_allowed_roots(vec![PathBuf::from("/srv/data")]);
    /// assert_eq!(fetcher.allowed_roots().map(<[_]>::len), Some(1));
    /// ```
    pub fn with_allowed_roots(roots: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            schemes: vec![FILE_SCHEME.to_owned()],
            allowed_roots: Some(roots.into_iter().collect()),
        }
    }

    /// The configured allowed roots, or `None` when any readable path is allowed.
    pub fn allowed_roots(&self) -> Option<&[PathBuf]> {
        self.allowed_roots.as_deref()
    }

    /// Parse a `file://` URI into a local [`PathBuf`].
    ///
    /// Rejects a missing path, a non-`localhost` authority, and malformed
    /// percent escapes with [`FetchError::InvalidAddress`].
    pub fn resolve(&self, uri: &str) -> Result<PathBuf, FetchError> {
        let rest = uri
            .strip_prefix("file://")
            .ok_or_else(|| FetchError::InvalidAddress(format!("not a file URI: `{uri}`")))?;

        // Everything up to the first `/` is the authority; `file:///x` has an
        // empty one. Only an empty or `localhost` authority is local.
        let (authority, path) = match rest.find('/') {
            Some(index) => (&rest[..index], &rest[index..]),
            None => (rest, ""),
        };
        if !authority.is_empty() && !authority.eq_ignore_ascii_case("localhost") {
            return Err(FetchError::InvalidAddress(format!(
                "file URI authority `{authority}` is not local: `{uri}`"
            )));
        }
        if path.is_empty() {
            return Err(FetchError::InvalidAddress(format!(
                "file URI has no path: `{uri}`"
            )));
        }

        Ok(PathBuf::from(percent_decode(path)?))
    }

    /// Resolve the source path and enforce the allowed-roots policy, returning
    /// the path to read (canonicalized when a policy is configured).
    async fn authorize(&self, source: PathBuf) -> Result<PathBuf, FetchError> {
        let Some(roots) = &self.allowed_roots else {
            return Ok(source);
        };
        let canonical = tokio::fs::canonicalize(&source).await?;
        for root in roots {
            // A root that cannot be canonicalized (usually because it does not
            // exist yet) simply matches nothing.
            if let Ok(root) = tokio::fs::canonicalize(root).await {
                if canonical.starts_with(&root) {
                    return Ok(canonical);
                }
            }
        }
        Err(FetchError::AccessDenied(format!(
            "{} is outside the configured file roots",
            source.display()
        )))
    }
}

impl Default for FileFetcher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Fetcher for FileFetcher {
    fn name(&self) -> &str {
        "file"
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
        let source = self.authorize(self.resolve(&request.address.uri)?).await?;
        let metadata = tokio::fs::metadata(&source).await?;
        if !metadata.is_file() {
            return Err(FetchError::Io(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!("not a regular file: {}", source.display()),
            )));
        }

        let temp = temp_path(&request.destination);
        match copy(&source, &temp, request, progress).await {
            Ok(fetched) => Ok(fetched),
            Err(error) => {
                // Never leave a partial copy behind, exactly like the HTTP fetcher.
                let _ = tokio::fs::remove_file(&temp).await;
                Err(error)
            }
        }
    }
}

/// Stream `source` into `temp`, verify its hash, then rename it into place.
async fn copy(
    source: &Path,
    temp: &Path,
    request: &FetchRequest,
    progress: &ProgressFn<'_>,
) -> Result<Fetched, FetchError> {
    if let Some(parent) = temp.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut reader = tokio::fs::File::open(source).await?;
    let mut writer = tokio::fs::File::create(temp).await?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; READ_CHUNK];
    let mut total = 0u64;

    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        writer.write_all(&buffer[..read]).await?;
        total += read as u64;
        progress(total);
    }
    writer.flush().await?;

    let digest = hex::encode(hasher.finalize());
    verify_sha256(request.expected_sha256.as_deref(), &digest)?;
    tokio::fs::rename(temp, &request.destination).await?;

    Ok(Fetched {
        path: request.destination.clone(),
        bytes: total,
        sha256: Some(digest),
        address: request.address.clone(),
    })
}

/// Decode the `%XX` escapes in a URI path into a UTF-8 string.
///
/// A malformed escape or a non-UTF-8 result is an [`FetchError::InvalidAddress`].
fn percent_decode(input: &str) -> Result<String, FetchError> {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err(FetchError::InvalidAddress(format!(
                    "truncated percent escape in file URI: `{input}`"
                )));
            }
            let high = hex_digit(bytes[index + 1], input)?;
            let low = hex_digit(bytes[index + 2], input)?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded)
        .map_err(|_| FetchError::InvalidAddress(format!("file URI is not valid UTF-8: `{input}`")))
}

/// The value of one hexadecimal digit, or an [`FetchError::InvalidAddress`].
fn hex_digit(byte: u8, input: &str) -> Result<u8, FetchError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(FetchError::InvalidAddress(format!(
            "invalid percent escape in file URI: `{input}`"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_absolute_paths_and_localhost() {
        let fetcher = FileFetcher::new();
        assert_eq!(
            fetcher.resolve("file:///data/report.pdf").expect("resolve"),
            PathBuf::from("/data/report.pdf")
        );
        assert_eq!(
            fetcher
                .resolve("file://localhost/data/report.pdf")
                .expect("resolve"),
            PathBuf::from("/data/report.pdf")
        );
        assert_eq!(
            fetcher
                .resolve("file://LOCALHOST/data/report.pdf")
                .expect("resolve"),
            PathBuf::from("/data/report.pdf")
        );
    }

    #[test]
    fn decodes_percent_escapes() {
        let fetcher = FileFetcher::new();
        assert_eq!(
            fetcher
                .resolve("file:///tmp/a%20b%2Fc.txt")
                .expect("resolve"),
            PathBuf::from("/tmp/a b/c.txt")
        );
    }

    #[test]
    fn rejects_non_local_authority_and_missing_path() {
        let fetcher = FileFetcher::new();
        for uri in [
            "file://server/share/file",
            "file://relative",
            "file://",
            "https://example.org/file",
        ] {
            assert!(
                matches!(fetcher.resolve(uri), Err(FetchError::InvalidAddress(_))),
                "`{uri}` should be rejected"
            );
        }
    }

    #[test]
    fn rejects_malformed_percent_escapes() {
        let fetcher = FileFetcher::new();
        for uri in [
            "file:///tmp/a%2.txt",
            "file:///tmp/a%zz.txt",
            "file:///tmp/a%",
        ] {
            assert!(
                matches!(fetcher.resolve(uri), Err(FetchError::InvalidAddress(_))),
                "`{uri}` should be rejected"
            );
        }
    }
}
