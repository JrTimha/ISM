use config::{Config, ConfigError, Environment, File};
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
#[allow(unused)]
pub struct ISMConfig {
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
    pub fn new(mode: &str) -> Result<Self, ConfigError> {
        //layering the different environment variables, default values first, overwritten by config files and env-vars
        let config = Config::builder()
            .add_source(File::with_name("default.config.toml"))
            .add_source(File::with_name(&format!("{mode}.config.toml")).required(false))
            .add_source(
                Environment::with_prefix("ism")
                    .prefix_separator("_")
                    .separator("__"),
            )
            .build()?;

        config.try_deserialize()
    }
}
