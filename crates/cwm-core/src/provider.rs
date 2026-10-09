use crate::types::{ErrorCode, ErrorInfo, FileQuery, FileResult};
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
pub trait SearchProvider: Send + Sync {
    fn name(&self) -> &str;
    fn search(&self, query: &FileQuery) -> ResultStream<'_>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct ProviderError {
    pub code: ErrorCode,
    pub message: String,
}

impl ProviderError {
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
