//! A canonical, versioned **configuration schema** for wiring providers and
//! fetchers into a client.
//!
//! Provider and fetcher wiring is a client concern, but every client otherwise
//! invents its own TOML shape. This module ships one documented shape so
//! clients, providers, examples, and tooling can agree on it. It is deliberately
//! small — only the wiring, never policy beyond ranking weights.
//!
//! The schema is **versioned** ([`CONFIG_VERSION`]). The serialized shape is a
//! stability contract: fields are only added within a schema version, new fields
//! are `#[serde(default)]` so older files still load, and unknown fields are
//! ignored so newer files stay readable by older builds (a *newer* explicit
//! `version` is rejected by [`Config::validate`]).
//!
//! ```no_run
//! # fn main() -> Result<(), ptox::ConfigError> {
//! use ptox::Config;
//!
//! let config = Config::load("~/.config/ptox/providers.toml")?;
//! let providers = config.providers();          // Vec<SubprocessProvider>
//! let registry = config.fetch_registry();      // http + configured commands
//! let engine = ptox::Engine::new(
//!     providers
//!         .into_iter()
//!         .map(|p| std::sync::Arc::new(p) as std::sync::Arc<dyn ptox::SearchProvider>)
//!         .collect(),
//! )
//! .with_weights(config.merge_options().provider_weights);
//! # let _ = (registry, engine);
//! # Ok(())
//! # }
//! ```
//!
//! A minimal config:
//!
//! ```toml
//! # Optional; defaults to the current schema version.
//! version = 1
//!
//! [[provider]]
//! name = "archive-org"
//! program = "node"
//! args = ["/opt/ptox/adapters/archive_org.js"]
//! env = { ARCHIVE_ORG_KEY = "…" }
//!
//! [ranking]
//! weights = { "archive-org" = 1.2 }
//!
//! [fetch.commands.magnet]
//! program = "aria2c"
//! args = ["--seed-time=0", "-d", "{dir}", "-o", "{name}", "{uri}"]
//! ```
//!
//! See `crates/ptox/examples/config.rs` for a runnable example.

use crate::merge::MergeOptions;
use crate::provider::SubprocessProvider;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[cfg(all(feature = "fetch", feature = "ipfs"))]
use crate::fetch::IpfsGatewayFetcher;
#[cfg(feature = "fetch")]
use crate::fetch::{CommandFetcher, FetchRegistry, HttpFetcher};

/// The newest configuration schema version this build understands.
pub const CONFIG_VERSION: u32 = 1;

fn default_version() -> u32 {
    CONFIG_VERSION
}

/// A whole client configuration: which providers to run, how to fetch, and how
/// to rank.
///
/// Build one with [`Config::load`] / [`Config::from_toml_str`], or construct it
/// in code and call [`Config::validate`]. See the [module docs](self) for the
/// TOML shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// Schema version. Defaults to [`CONFIG_VERSION`] when omitted.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Provider commands to fan out to.
    #[serde(default, rename = "provider", skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<ProviderConfig>,
    /// Retrieval wiring.
    #[serde(default)]
    pub fetch: FetchConfig,
    /// Ranking policy.
    #[serde(default)]
    pub ranking: RankingConfig,
}

/// One external (NDJSON) provider command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Stable provider name, used for attribution, capabilities, and weights.
    pub name: String,
    /// The program to run.
    pub program: String,
    /// Command-line arguments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Extra environment variables for the child process.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

impl ProviderConfig {
    /// Build the [`SubprocessProvider`] this entry describes.
    pub fn to_provider(&self) -> SubprocessProvider {
        let mut provider = SubprocessProvider::new(self.name.as_str(), self.program.as_str())
            .args(self.args.clone());
        if !self.env.is_empty() {
            provider = provider.envs(self.env.clone());
        }
        provider
    }
}

/// Retrieval wiring.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FetchConfig {
    /// External commands that retrieve a scheme, keyed by scheme
    /// (e.g. `magnet`). Each command must create its `{dest}` (see
    /// [`crate::fetch::CommandFetcher`]).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub commands: BTreeMap<String, CommandConfig>,
}

/// One scheme delegated to an external fetch command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandConfig {
    /// The program to run.
    pub program: String,
    /// Arguments, with `{uri}`/`{dest}`/`{dir}`/`{name}` placeholders.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

/// Ranking policy.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RankingConfig {
    /// Per-provider weight (default `1.0` in [`crate::merge`]).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub weights: BTreeMap<String, f64>,
}

/// Why a configuration could not be read, parsed, or validated.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The config file could not be read.
    #[error("cannot read config at {path}: {source}")]
    Read {
        /// The path that failed to read.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The config was not valid TOML / did not match the schema.
    #[error("invalid config: {0}")]
    Parse(#[from] toml::de::Error),
    /// The config could not be serialized back to TOML.
    #[error("cannot serialize config: {0}")]
    Serialize(#[from] toml::ser::Error),
    /// The file declares a schema version this build does not understand.
    #[error("unsupported config version {found} (this build supports up to {supported})")]
    UnsupportedVersion {
        /// The version found in the file.
        found: u32,
        /// The newest version this build supports.
        supported: u32,
    },
    /// The config parsed but is semantically invalid.
    #[error("invalid config: {0}")]
    Invalid(String),
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            providers: Vec::new(),
            fetch: FetchConfig::default(),
            ranking: RankingConfig::default(),
        }
    }
}

impl Config {
    /// An empty config at the current schema version.
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse and validate a TOML string.
    pub fn from_toml_str(text: &str) -> Result<Self, ConfigError> {
        let config: Config = toml::from_str(text)?;
        config.validate()?;
        Ok(config)
    }

    /// Read, parse, and validate the TOML config at `path`.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_toml_str(&text)
    }

    /// Serialize this config back to pretty TOML. Round-trips through
    /// [`Config::from_toml_str`].
    pub fn to_toml_string(&self) -> Result<String, ConfigError> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Check the schema version and the semantics that TOML itself cannot
    /// express: unique, non-empty provider names and programs, non-empty fetch
    /// command schemes and programs, and finite, non-negative weights.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.version == 0 || self.version > CONFIG_VERSION {
            return Err(ConfigError::UnsupportedVersion {
                found: self.version,
                supported: CONFIG_VERSION,
            });
        }

        let mut names = BTreeSet::new();
        for provider in &self.providers {
            if provider.name.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "provider name must not be empty".to_owned(),
                ));
            }
            if !names.insert(provider.name.as_str()) {
                return Err(ConfigError::Invalid(format!(
                    "duplicate provider name `{}`",
                    provider.name
                )));
            }
            if provider.program.trim().is_empty() {
                return Err(ConfigError::Invalid(format!(
                    "provider `{}` must set a program",
                    provider.name
                )));
            }
            if provider.env.keys().any(|key| key.trim().is_empty()) {
                return Err(ConfigError::Invalid(format!(
                    "provider `{}` has an empty environment variable name",
                    provider.name
                )));
            }
        }

        for (scheme, command) in &self.fetch.commands {
            if scheme.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "fetch command scheme must not be empty".to_owned(),
                ));
            }
            if command.program.trim().is_empty() {
                return Err(ConfigError::Invalid(format!(
                    "fetch command `{scheme}` must set a program"
                )));
            }
        }

        for (provider, weight) in &self.ranking.weights {
            if !weight.is_finite() || *weight < 0.0 {
                return Err(ConfigError::Invalid(format!(
                    "ranking weight for `{provider}` must be a finite, non-negative number"
                )));
            }
        }

        Ok(())
    }

    /// The configured providers as ready-to-run [`SubprocessProvider`]s.
    pub fn providers(&self) -> Vec<SubprocessProvider> {
        self.providers
            .iter()
            .map(ProviderConfig::to_provider)
            .collect()
    }

    /// The ranking policy as [`MergeOptions`] for [`crate::merge`] /
    /// [`crate::Engine::with_weights`].
    pub fn merge_options(&self) -> MergeOptions {
        MergeOptions {
            provider_weights: self
                .ranking
                .weights
                .iter()
                .map(|(provider, weight)| (provider.clone(), *weight))
                .collect(),
        }
    }

    /// Build a [`FetchRegistry`] from the config: built-in `http`/`https` (and
    /// the `ipfs://` gateway when the `ipfs` feature is on), plus one
    /// [`CommandFetcher`] per `[fetch.commands.<scheme>]`.
    ///
    /// Configured commands are registered **before** the built-ins, so an
    /// explicitly configured scheme (for example a local `ipfs get`) takes
    /// precedence over the built-in handler for that scheme.
    #[cfg(feature = "fetch")]
    pub fn fetch_registry(&self) -> FetchRegistry {
        let mut registry = FetchRegistry::new();
        for (scheme, command) in &self.fetch.commands {
            registry.register(std::sync::Arc::new(CommandFetcher::new(
                scheme.clone(),
                vec![scheme.clone()],
                command.program.clone(),
                command.args.clone(),
            )));
        }
        registry.register(std::sync::Arc::new(HttpFetcher::new()));
        #[cfg(feature = "ipfs")]
        registry.register(std::sync::Arc::new(
            IpfsGatewayFetcher::with_default_gateway(),
        ));
        registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::SearchProvider;

    fn sample() -> Config {
        let mut env = BTreeMap::new();
        env.insert("MOCKNET_URL".to_owned(), "http://127.0.0.1:8642".to_owned());
        let mut weights = BTreeMap::new();
        weights.insert("local".to_owned(), 1.5);
        let mut commands = BTreeMap::new();
        commands.insert(
            "magnet".to_owned(),
            CommandConfig {
                program: "aria2c".to_owned(),
                args: vec!["{uri}".to_owned(), "{dest}".to_owned()],
            },
        );
        Config {
            version: CONFIG_VERSION,
            providers: vec![ProviderConfig {
                name: "local".to_owned(),
                program: "node".to_owned(),
                args: vec!["adapter.js".to_owned()],
                env,
            }],
            fetch: FetchConfig { commands },
            ranking: RankingConfig { weights },
        }
    }

    #[test]
    fn round_trips_through_toml() {
        let config = sample();
        let text = config.to_toml_string().expect("serialize");
        let parsed = Config::from_toml_str(&text).expect("parse");
        assert_eq!(parsed, config);
    }

    #[test]
    fn version_defaults_to_current() {
        let config = Config::from_toml_str("[[provider]]\nname = \"a\"\nprogram = \"p\"").unwrap();
        assert_eq!(config.version, CONFIG_VERSION);
    }

    #[test]
    fn parses_the_canonical_shape() {
        let config = Config::from_toml_str(
            r#"
version = 1

[[provider]]
name = "local"
program = "node"
args = ["adapter.js"]
env = { MOCKNET_URL = "http://localhost:8642" }

[ranking]
weights = { local = 1.5 }

[fetch.commands.magnet]
program = "aria2c"
args = ["-d", "{dir}", "{uri}"]
"#,
        )
        .expect("valid config");
        assert_eq!(config.providers.len(), 1);
        let provider = &config.providers[0];
        assert_eq!(provider.name, "local");
        assert_eq!(provider.args, vec!["adapter.js"]);
        assert_eq!(provider.env["MOCKNET_URL"], "http://localhost:8642");
        assert_eq!(config.ranking.weights["local"], 1.5);
        assert_eq!(config.fetch.commands["magnet"].program, "aria2c");
    }

    #[test]
    fn rejects_a_newer_version() {
        let error = Config::from_toml_str("version = 99").unwrap_err();
        assert!(matches!(
            error,
            ConfigError::UnsupportedVersion { found: 99, .. }
        ));
    }

    #[test]
    fn rejects_duplicate_provider_names() {
        let error = Config::from_toml_str(
            "[[provider]]\nname = \"a\"\nprogram = \"p\"\n\n[[provider]]\nname = \"a\"\nprogram = \"q\"",
        )
        .unwrap_err();
        assert!(matches!(error, ConfigError::Invalid(_)));
    }

    #[test]
    fn rejects_empty_program() {
        let error =
            Config::from_toml_str("[[provider]]\nname = \"a\"\nprogram = \"  \"").unwrap_err();
        assert!(matches!(error, ConfigError::Invalid(_)));
    }

    #[test]
    fn rejects_invalid_weights() {
        for weight in ["-1.0", "nan", "inf"] {
            let text = format!("[ranking]\nweights = {{ a = {weight} }}");
            let error = Config::from_toml_str(&text).unwrap_err();
            assert!(matches!(error, ConfigError::Invalid(_)), "{weight}");
        }
    }

    #[test]
    fn unknown_fields_are_ignored_for_forward_compatibility() {
        let config = Config::from_toml_str(
            "future = true\n[[provider]]\nname = \"a\"\nprogram = \"p\"\nextra = 3",
        )
        .expect("unknown fields ignored");
        assert_eq!(config.providers[0].name, "a");
    }

    #[test]
    fn builds_providers_and_merge_options() {
        let config = sample();
        let providers = config.providers();
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].name(), "local");
        assert_eq!(config.merge_options().provider_weights["local"], 1.5);
    }

    #[cfg(feature = "fetch")]
    #[test]
    fn builds_a_fetch_registry() {
        let registry = sample().fetch_registry();
        assert!(registry.fetcher_for("magnet").is_some());
        assert!(registry.fetcher_for("https").is_some());
    }
}
