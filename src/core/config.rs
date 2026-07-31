use config::{Config, ConfigError, Environment, File};
use serde::Deserialize;
use std::env;

/// Environment variable selecting which `{mode}.config.toml` is layered on top of the defaults.
const MODE_ENV: &str = "ISM_MODE";

/// Mode used when [`MODE_ENV`] is unset.
const DEFAULT_MODE: &str = "development";

#[derive(Deserialize, Debug, Clone)]
#[allow(unused)]
pub struct ISMConfig {
    /// The mode this configuration was loaded for.
    ///
    /// Not read from the files — it is what *selected* them. Recorded here so that everything
    /// which needs to know the mode reads it off the config instead of going back to the
    /// environment with its own copy of the default.
    #[serde(skip)]
    pub run_mode: String,

    pub ism_port: u16,
    pub ism_url: String,
    pub use_kafka: bool,
    pub log_level: String,
    pub cors_origin: String,
    pub redis_cache_url: Option<String>,
    pub room_db_config: RoomDbConfig,
    pub object_db_config: ObjectStorageConfig,
    pub token_issuer: TokenIssuer,
    pub kafka_config: KafkaConfig,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ObjectStorageConfig {
    pub access_key: String,
    pub storage_url: String,
    pub secret_key: String,
    pub bucket_name: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct RoomDbConfig {
    pub db_host: String,
    pub db_port: u16,
    pub db_user: String,
    pub db_password: String,
    pub db_name: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct TokenIssuer {
    pub iss_host: String,
    pub iss_realm: String,

    /// Accepted values of the JWT `aud` claim.
    ///
    /// The default `["account"]` is what Keycloak's built-in `audience-resolve` mapper adds to
    /// *every* access token of *every* client in the realm — it therefore does not scope access to
    /// a particular client. Configure a dedicated audience mapper in Keycloak and set this to that
    /// audience to make the check meaningful.
    #[serde(default = "default_expected_audiences")]
    pub expected_audiences: Vec<String>,

    /// Accepted values of the JWT `azp` (authorized party) claim. Empty disables the check.
    ///
    /// Set this to your application's Keycloak client id to reject tokens minted for other clients
    /// in the same realm.
    #[serde(default)]
    pub expected_azp: Vec<String>,

    /// Signature algorithms accepted for incoming tokens.
    ///
    /// This is a fixed allow-list: the algorithm named in the (attacker-controlled) JWT header is
    /// never used to decide which algorithms are acceptable.
    #[serde(default = "default_allowed_algorithms")]
    pub allowed_algorithms: Vec<String>,

    /// Minimum time between two OIDC discoveries, in seconds.
    ///
    /// Rate-limits on-demand refreshes so a flood of invalid tokens cannot turn into a request
    /// storm against Keycloak.
    #[serde(default = "default_jwks_min_refresh_interval_secs")]
    pub jwks_min_refresh_interval_secs: u64,
}

fn default_expected_audiences() -> Vec<String> {
    vec![String::from("account")]
}

fn default_allowed_algorithms() -> Vec<String> {
    vec![String::from("RS256")]
}

fn default_jwks_min_refresh_interval_secs() -> u64 {
    30
}

#[derive(Deserialize, Debug, Clone)]
pub struct KafkaConfig {
    pub bootstrap_host: String,
    pub bootstrap_port: u16,
    pub topic: String,
    pub client_id: String,
    pub partition: Vec<i32>,
    pub consumer_group: String,
}

//examples: https://github.com/rust-cli/config-rs/blob/main/examples/hierarchical-env/settings.rs
impl ISMConfig {
    /// Loads the configuration.
    ///
    /// Reads `ISM_MODE` itself — picking the mode *is* part of loading the configuration, so there
    /// is nothing for a caller to do first.
    ///
    /// Sources are layered, each overriding the one before:
    /// 1. `default.config.toml` (required)
    /// 2. `{mode}.config.toml` (optional)
    /// 3. `ISM_*` environment variables, `__` separating nesting levels
    ///    (`ISM_ROOM_DB_CONFIG__DB_HOST`)
    ///
    /// Every field is reachable from step 3, including the flat ones: `ISM_LOG_LEVEL` sets
    /// `log_level`, `ISM_CORS_ORIGIN` sets `cors_origin`, and so on. Nothing needs to read an
    /// `ISM_*` variable by hand.
    pub fn new() -> Result<Self, ConfigError> {
        let run_mode = env::var(MODE_ENV).unwrap_or_else(|_| DEFAULT_MODE.to_owned());

        //layering the different environment variables, default values first, overwritten by config files and env-vars
        let config = Config::builder()
            .add_source(File::with_name("default.config.toml"))
            .add_source(File::with_name(&format!("{run_mode}.config.toml")).required(false))
            .add_source(env_source())
            .build()?;

        let mut config: ISMConfig = config.try_deserialize()?;
        config.run_mode = run_mode;
        Ok(config)
    }
}

/// The `ISM_*` environment layer.
///
/// Extracted so the tests below exercise the exact source [`ISMConfig::new`] uses, via
/// `Environment::source`, instead of mutating the process environment.
fn env_source() -> Environment {
    Environment::with_prefix("ism")
        .prefix_separator("_")
        .separator("__")
        // An env var that is set but empty means "unset", not "override with nothing". Container
        // tooling forwards undeclared variables as empty strings, so without this a stray
        // `ISM_LOG_LEVEL=` in a compose file would blank out a configured value rather than leave
        // it alone.
        .ignore_empty(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::Map;

    /// Loads only the environment layer, from an injected map rather than the real environment.
    fn from_env(vars: &[(&str, &str)]) -> config::Config {
        let source: Map<String, String> = vars
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();

        Config::builder()
            .add_source(env_source().source(Some(source)))
            .build()
            .expect("the environment layer alone always builds")
    }

    #[test]
    fn flat_fields_are_reachable_without_a_nesting_separator() {
        // This is what lets `main` drop its hand-rolled `ISM_LOG_LEVEL` lookup: the variable is
        // nothing special, it lands on `log_level` like any other field.
        let config = from_env(&[("ISM_LOG_LEVEL", "info,sqlx=warn")]);

        assert_eq!(config.get_string("log_level").unwrap(), "info,sqlx=warn");
    }

    #[test]
    fn nested_fields_use_the_double_underscore_separator() {
        // A single underscore stays part of the field name, so `db_host` survives intact and only
        // `__` descends into the section.
        let config = from_env(&[("ISM_ROOM_DB_CONFIG__DB_HOST", "postgres.internal")]);

        assert_eq!(
            config.get_string("room_db_config.db_host").unwrap(),
            "postgres.internal"
        );
    }

    #[test]
    fn an_empty_variable_does_not_override() {
        // Compose forwards an undeclared variable as an empty string; that must not blank out a
        // value the config file set.
        let config = from_env(&[("ISM_LOG_LEVEL", "")]);

        assert!(
            config.get_string("log_level").is_err(),
            "an empty ISM_LOG_LEVEL must be treated as unset, leaving the configured value in place"
        );
    }
}
