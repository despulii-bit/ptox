use crate::search::capabilities::Capabilities;
use crate::types::{ErrorCode, ErrorInfo, FileQuery, FileResult};
use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

/// A stream of results from one provider. Errors are per-item and never global.
pub type ResultStream<'a> =
    Pin<Box<dyn Stream<Item = Result<FileResult, ProviderError>> + Send + 'a>>;

/// One network (or index) adapter.
///
/// All protocol, proxy, auth, and transport detail lives inside the
/// implementation and is injected at construction, never per query.
#[async_trait]
pub trait SearchProvider: Send + Sync {
    /// The provider's stable name; used for attribution and ranking weights.
    fn name(&self) -> &str;

    /// What this provider can do. Defaults to universal (accepts anything).
    async fn capabilities(&self) -> Capabilities {
        Capabilities::universal()
    }

    /// Stream results for `query`. Errors are per-item, never global.
    fn search(&self, query: &FileQuery) -> ResultStream<'_>;
}

/// An error a provider attaches to a single item or query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct ProviderError {
    /// The error category, mapped to a [`crate::types::ErrorInfo`] on the fanout.
    pub code: ErrorCode,
    /// A short, non-empty explanation. Never carries secrets.
    pub message: String,
}

impl ProviderError {
    /// Build an error from a code and message.
    pub fn new(code: impl Into<ErrorCode>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl From<ProviderError> for ErrorInfo {
    fn from(error: ProviderError) -> Self {
        ErrorInfo {
            code: error.code,
            message: error.message,
        }
    }
}
