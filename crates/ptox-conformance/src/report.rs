use serde::Serialize;

/// One conformance check and its outcome.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

/// The full set of checks run against one provider.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub provider: String,
    pub checks: Vec<Check>,
}

impl Report {
    pub fn new(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            checks: Vec::new(),
        }
    }

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

    pub fn failures(&self) -> impl Iterator<Item = &Check> {
        self.checks.iter().filter(|check| !check.passed)
    }
}
