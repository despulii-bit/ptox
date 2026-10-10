//! A runnable walkthrough of the canonical configuration schema.
//!
//! It parses a TOML config, validates it, and turns it into the pieces a client
//! needs: a ready-to-run `Engine` (providers + ranking + fetchers), plus the
//! individual providers, fetch registry, and ranking options when a client wants
//! them separately.
//!
//! ```sh
//! cargo run -p ptox --example config
//! ```
//!
//! See the [`ptox::config`] module for the schema and
//! `docs/configuration.md` for the field-by-field guide.

use ptox::{Config, CONFIG_VERSION};

/// A complete config exercising every section. In a real client this would be
/// read from disk with `Config::load`; here it is inline so the example is
/// self-contained and offline.
const EXAMPLE: &str = r#"
# Optional: defaults to the current schema version.
version = 1

[[provider]]
name = "archive-org"
program = "node"
args = ["/opt/ptox/adapters/archive_org.js"]
env = { ARCHIVE_ORG_TIMEOUT = "10" }

[[provider]]
name = "local"
program = "node"
args = ["/opt/ptox/adapters/local.js"]

[ranking]
weights = { "archive-org" = 1.2 }

[fetch.commands.magnet]
program = "aria2c"
args = ["--seed-time=0", "-d", "{dir}", "-o", "{name}", "{uri}"]
"#;

fn main() -> Result<(), ptox::ConfigError> {
    let config = Config::from_toml_str(EXAMPLE)?;

    println!(
        "config schema version: {} (current: {})",
        config.version, CONFIG_VERSION
    );

    let providers = config.providers();
    for provider in &providers {
        use ptox::SearchProvider;
        println!("provider: {}", provider.name());
    }

    // The one-call path: providers, ranking weights, and fetchers wired up.
    let engine = config.engine();
    println!("engine providers: {}", engine.providers().len());

    let registry = config.fetch_registry();
    println!(
        "fetch schemes: http={} magnet={}",
        registry.fetcher_for("http").is_some(),
        registry.fetcher_for("magnet").is_some(),
    );

    let options = config.merge_options();
    println!("ranking weights: {:?}", options.provider_weights);

    // The config also round-trips back to TOML (useful for `ptox config init`).
    let text = config.to_toml_string()?;
    debug_assert_eq!(Config::from_toml_str(&text)?, config);
    println!("round-trip: ok");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ptox::SearchProvider;

    #[test]
    fn example_config_is_valid_and_complete() {
        let config = Config::from_toml_str(EXAMPLE).expect("example config is valid");
        assert_eq!(config.version, CONFIG_VERSION);
        assert_eq!(config.providers.len(), 2);
        let names: Vec<String> = config
            .providers()
            .iter()
            .map(|p| p.name().to_owned())
            .collect();
        assert_eq!(names, vec!["archive-org", "local"]);
        assert!(config.fetch.commands.contains_key("magnet"));
    }

    #[test]
    fn example_config_builds_a_registry() {
        let registry = Config::from_toml_str(EXAMPLE).unwrap().fetch_registry();
        assert!(registry.fetcher_for("https").is_some());
        assert!(registry.fetcher_for("magnet").is_some());
    }

    #[test]
    fn example_config_builds_an_engine() {
        let engine = Config::from_toml_str(EXAMPLE).unwrap().engine();
        let names: Vec<&str> = engine
            .providers()
            .iter()
            .map(|provider| provider.name())
            .collect();
        assert_eq!(names, vec!["archive-org", "local"]);
    }
}
