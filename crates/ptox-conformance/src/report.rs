use serde::Serialize;

/// One conformance check and its outcome.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    /// The check's stable name.
    pub name: String,
    /// Whether the check passed.
    pub passed: bool,
    /// A human-readable result summary.
    pub detail: String,
}

/// The full set of checks run against one provider.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// The provider under test.
    pub provider: String,
    /// Every check that was run.
    pub checks: Vec<Check>,
}

impl Report {
    /// An empty report for `provider`.
    pub fn new(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            checks: Vec::new(),
        }
    }

    /// Record a check result.
    pub fn push(&mut self, name: impl Into<String>, passed: bool, detail: impl Into<String>) {
        self.checks.push(Check {
            name: name.into(),
            passed,
            detail: detail.into(),
        });
    }

    /// True when every check passed.
    pub fn passed(&self) -> bool {
        self.checks.iter().all(|check| check.passed)
    }

    /// The checks that did not pass.
    pub fn failures(&self) -> impl Iterator<Item = &Check> {
        self.checks.iter().filter(|check| !check.passed)
    }
}
