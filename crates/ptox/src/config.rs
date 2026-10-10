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
//! let config = ptox::Config::load("~/.config/ptox/providers.toml")?;
//! let engine = config.engine();                // providers + weights + fetchers
//! let registry = config.fetch_registry();      // the fetchers on their own
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

use crate::engine::Engine;
use crate::merge::MergeOptions;
use crate::provider::SubprocessProvider;
use crate::search::{Options, SearchProvider};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
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
    /// A `${VAR}` / `$VAR` reference named an environment variable that is not
    /// set.
    #[error("environment variable `{name}` is referenced but not set")]
    MissingEnv {
        /// The referenced variable name.
        name: String,
    },
    /// A malformed environment reference (e.g. an unclosed `${` or an invalid
    /// name).
    #[error("malformed environment reference in `{value}`")]
    InvalidEnvSyntax {
        /// The value that contained the malformed reference.
        value: String,
    },
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

    /// Read, parse, expand, and validate the TOML config at `path`.
    ///
    /// Environment references ([`Config::expand_env`]) are resolved here with
    /// the process environment, so a config can be shared across machines with
    /// `${HOME}`, `${DATA_DIR}`, and similar. Use [`Config::from_toml_str`] to
    /// parse without touching the environment.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let config = Self::from_toml_str(&text)?.expand_env()?;
        config.validate()?;
        Ok(config)
    }

    /// Serialize this config back to pretty TOML. Round-trips through
    /// [`Config::from_toml_str`].
    pub fn to_toml_string(&self) -> Result<String, ConfigError> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Expand `${VAR}` / `$VAR` references in provider and fetch-command
    /// `program` / `args` and in provider `env` values, using the process
    /// environment.
    ///
    /// `$VAR` and `${VAR}` both work (braces are unambiguous when the name is
    /// followed by other text); `$$` is a literal `$`. A reference to an unset
    /// variable is an error, as is a malformed reference (an unclosed `${` or a
    /// name that is not `[A-Za-z_][A-Za-z0-9_]*`). Environment-variable *keys*
    /// are not expanded.
    ///
    /// [`Config::load`] applies this automatically. [`Config::from_toml_str`]
    /// parses literally, so call this yourself when you embed a config string.
    pub fn expand_env(&self) -> Result<Config, ConfigError> {
        self.expand_env_with(|name| std::env::var(name).ok())
    }

    /// Like [`Config::expand_env`], but resolve names with `lookup` instead of
    /// the process environment (for tests or a custom environment).
    pub fn expand_env_with<F>(&self, lookup: F) -> Result<Config, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let mut config = self.clone();
        for provider in &mut config.providers {
            provider.program = expand(&provider.program, &lookup)?;
            for arg in &mut provider.args {
                *arg = expand(arg, &lookup)?;
            }
            for value in provider.env.values_mut() {
                *value = expand(value, &lookup)?;
            }
        }
        for command in config.fetch.commands.values_mut() {
            command.program = expand(&command.program, &lookup)?;
            for arg in &mut command.args {
                *arg = expand(arg, &lookup)?;
            }
        }
        Ok(config)
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

    /// Build a ready-to-run [`Engine`] from the config: the configured
    /// providers, the ranking weights, and — with the `fetch` feature — the
    /// configured [`FetchRegistry`].
    ///
    /// This is the canonical `load → search` path, replacing the hand-wiring of
    /// [`Engine::new`] / [`Engine::with_options`] / [`Engine::with_weights`] /
    /// [`Engine::with_fetchers`] that every client otherwise repeats:
    ///
    /// ```no_run
    /// # async fn run() -> Result<(), ptox::ConfigError> {
    /// let config = ptox::Config::load("~/.config/ptox/providers.toml")?;
    /// let engine = config.engine();
    /// let report = engine.search(ptox::FileQuery::new("annual report")).await;
    /// # let _ = report;
    /// # Ok(())
    /// # }
    /// ```
    pub fn engine(&self) -> Engine {
        self.engine_with_options(Options::default())
    }

    /// Like [`Config::engine`], but with explicit search [`Options`] (provider
    /// timeout, global limit/offset, concurrency). The config does not carry
    /// these policy knobs — they belong to the client.
    pub fn engine_with_options(&self, options: Options) -> Engine {
        let providers: Vec<Arc<dyn SearchProvider>> = self
            .providers()
            .into_iter()
            .map(|provider| Arc::new(provider) as Arc<dyn SearchProvider>)
            .collect();
        Engine::new(providers)
            .with_options(options)
            .with_weights(self.merge_options().provider_weights)
            .with_fetchers(self.fetch_registry())
    }

    /// Build a [`FetchRegistry`] from the config: built-in `http`/`https` (and
    /// the `ipfs://` gateway when the `ipfs` feature is on, and the `file`
    /// fetcher when the `file` feature is on), plus one
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
        #[cfg(feature = "file")]
        registry.register(std::sync::Arc::new(crate::fetch::FileFetcher::new()));
        registry
    }
}

/// Expand `$VAR` / `${VAR}` references (and `$$` → `$`) in `value`, resolving
/// names with `lookup`. See [`Config::expand_env`].
fn expand(value: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Result<String, ConfigError> {
    if !value.contains('$') {
        return Ok(value.to_owned());
    }

    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '$' {
            out.push(character);
            continue;
        }
        match chars.peek().copied() {
            Some('$') => {
                chars.next();
                out.push('$');
            }
            Some('{') => {
                chars.next();
                let mut name = String::new();
                let mut closed = false;
                for inner in chars.by_ref() {
                    if inner == '}' {
                        closed = true;
                        break;
                    }
                    name.push(inner);
                }
                if !closed || !is_valid_env_name(&name) {
                    return Err(ConfigError::InvalidEnvSyntax {
                        value: value.to_owned(),
                    });
                }
                out.push_str(&resolve(&name, lookup)?);
            }
            Some(start) if start.is_ascii_alphabetic() || start == '_' => {
                let mut name = String::new();
                while let Some(&next) = chars.peek() {
                    if next.is_ascii_alphanumeric() || next == '_' {
                        name.push(next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                out.push_str(&resolve(&name, lookup)?);
            }
            _ => out.push('$'),
        }
    }
    Ok(out)
}

fn resolve(name: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Result<String, ConfigError> {
    lookup(name).ok_or_else(|| ConfigError::MissingEnv {
        name: name.to_owned(),
    })
}

fn is_valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
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

    #[test]
    fn engine_wires_configured_providers() {
        let engine = sample().engine();
        let names: Vec<&str> = engine
            .providers()
            .iter()
            .map(|provider| provider.name())
            .collect();
        assert_eq!(names, vec!["local"]);
    }

    #[test]
    fn engine_with_options_keeps_configured_providers() {
        let options = Options {
            limit: Some(3),
            ..Options::default()
        };
        let engine = sample().engine_with_options(options);
        assert_eq!(engine.providers().len(), 1);
    }

    #[test]
    fn engine_of_empty_config_has_no_providers() {
        let engine = Config::new().engine();
        assert!(engine.providers().is_empty());
    }

    fn provider_with(program: &str, args: Vec<&str>, env: BTreeMap<String, String>) -> Config {
        Config {
            version: CONFIG_VERSION,
            providers: vec![ProviderConfig {
                name: "p".to_owned(),
                program: program.to_owned(),
                args: args.into_iter().map(str::to_owned).collect(),
                env,
            }],
            fetch: FetchConfig::default(),
            ranking: RankingConfig::default(),
        }
    }

    #[test]
    fn expands_variables_in_program_args_and_env() {
        let mut env = BTreeMap::new();
        env.insert("API_TOKEN".to_owned(), "${TOKEN}".to_owned());
        let config = provider_with("${BIN}", vec!["--dir=$DATA_DIR", "literal$$dollar"], env);

        let expanded = config
            .expand_env_with(|name| match name {
                "BIN" => Some("/usr/bin/provider".to_owned()),
                "DATA_DIR" => Some("/srv/data".to_owned()),
                "TOKEN" => Some("s3cr3t".to_owned()),
                _ => None,
            })
            .expect("expand");

        let provider = &expanded.providers[0];
        assert_eq!(provider.program, "/usr/bin/provider");
        assert_eq!(provider.args, vec!["--dir=/srv/data", "literal$dollar"]);
        assert_eq!(provider.env["API_TOKEN"], "s3cr3t");
    }

    #[test]
    fn expands_fetch_command_program_and_args() {
        let mut commands = BTreeMap::new();
        commands.insert(
            "magnet".to_owned(),
            CommandConfig {
                program: "${ARIA}".to_owned(),
                args: vec!["-d".to_owned(), "${DIR}".to_owned(), "{uri}".to_owned()],
            },
        );
        let config = Config {
            version: CONFIG_VERSION,
            providers: Vec::new(),
            fetch: FetchConfig { commands },
            ranking: RankingConfig::default(),
        };

        let expanded = config
            .expand_env_with(|name| match name {
                "ARIA" => Some("aria2c".to_owned()),
                "DIR" => Some("/tmp".to_owned()),
                _ => None,
            })
            .expect("expand");

        let command = &expanded.fetch.commands["magnet"];
        assert_eq!(command.program, "aria2c");
        assert_eq!(command.args, vec!["-d", "/tmp", "{uri}"]);
    }

    #[test]
    fn errors_on_a_missing_variable() {
        let error = provider_with("$NOPE", vec![], BTreeMap::new())
            .expand_env_with(|_| None)
            .unwrap_err();
        assert!(matches!(error, ConfigError::MissingEnv { name } if name == "NOPE"));
    }

    #[test]
    fn rejects_malformed_environment_references() {
        for bad in ["${UNCLOSED", "${1BAD}", "${}", "${BAD-NAME}"] {
            let error = provider_with(bad, vec![], BTreeMap::new())
                .expand_env_with(|_| Some("x".to_owned()))
                .unwrap_err();
            assert!(
                matches!(error, ConfigError::InvalidEnvSyntax { .. }),
                "{bad}"
            );
        }
    }

    #[test]
    fn treats_a_lone_dollar_as_literal() {
        let expanded = provider_with("cost$5", vec![], BTreeMap::new())
            .expand_env_with(|_| Some("x".to_owned()))
            .expect("expand");
        assert_eq!(expanded.providers[0].program, "cost$5");
    }

    #[test]
    fn load_expands_the_process_environment() {
        let var = format!("PTOX_CONFIG_TEST_{}", std::process::id());
        std::env::set_var(&var, "/opt/bin");

        let path =
            std::env::temp_dir().join(format!("ptox-config-expand-{}.toml", std::process::id()));
        std::fs::write(
            &path,
            format!("[[provider]]\nname = \"p\"\nprogram = \"${{{var}}}/provider\"\n"),
        )
        .expect("write temp config");

        let config = Config::load(&path).expect("load");
        assert_eq!(config.providers[0].program, "/opt/bin/provider");

        let _ = std::fs::remove_file(&path);
        std::env::remove_var(&var);
    }
}
