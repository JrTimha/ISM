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
    pub user_db_config: UserDbConfig,
    pub object_db_config: ObjectStorageConfig,
    pub message_db_config: MessageDbConfig,
    pub token_issuer: TokenIssuer,
    pub kafka_config: KafkaConfig
}

#[derive(Deserialize, Debug, Clone)]
pub struct ObjectStorageConfig {
    pub access_key: String,
    pub storage_url: String,
    pub secret_key: String,
    pub bucket_name: String
}

#[derive(Deserialize, Debug, Clone)]
pub struct MessageDbConfig {
    pub db_url: String,
    pub db_user: String,
    pub db_password: String,
    pub db_keyspace: String,
    pub with_db_init: bool
}

#[derive(Deserialize, Debug, Clone)]
pub struct UserDbConfig {
    pub db_host: String,
    pub db_port: u16,
    pub db_user: String,
    pub db_password: String,
    pub db_name: String
}

#[derive(Deserialize, Debug, Clone)]
pub struct TokenIssuer {
    pub iss_host: String,
    pub iss_realm: String,
    pub valid_admin_client: Option<String>
}

#[derive(Deserialize, Debug, Clone)]
pub struct KafkaConfig {
    pub bootstrap_host: String,
    pub bootstrap_port: u16,
    pub topic: String,
    pub client_id: String,
    pub partition: Vec<i32>,
    pub consumer_group: String
}

//examples: https://github.com/rust-cli/config-rs/blob/main/examples/hierarchical-env/settings.rs
impl ISMConfig {

    pub fn new(mode: &str) -> Result<Self, ConfigError> {
        //layering the different environment variables, default values first, overwritten by config files and env-vars
        let config = Config::builder()
            .add_source(File::with_name("default.config.toml"))
            .add_source(File::with_name(&format!("{mode}.config.toml")).required(false))
            .add_source(Environment::with_prefix("ism").prefix_separator("_").separator("__"))
            .build()?;

        config.try_deserialize()
    }
}