use config::{Config, ConfigError, Environment, File};
use serde::Deserialize;


#[derive(Deserialize, Debug, Clone)]
#[allow(unused)]
pub struct ISMConfig {
    pub ism_port: u16,
    pub ism_url: String,
    pub log_level: String,
    pub cors_origin: String,
    pub user_db_config: UserDbConfig,
    pub message_db_config: MessageDbConfig,
    pub token_issuer: TokenIssuer
}

#[derive(Deserialize, Debug, Clone)]
pub struct MessageDbConfig {
    pub db_url: String,
    pub db_user: String,
    pub db_password: String,
    pub db_keyspace: String,
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
    pub iss_realm: String
}

//examples: https://github.com/rust-cli/config-rs/blob/main/examples/hierarchical-env/settings.rs
impl ISMConfig {
    pub fn new_config(mode: &str) -> Result<Self, ConfigError> {
        //layering the different environment variables, default values first, overwritten by config files and env-vars
        let config = Config::builder()
            .add_source(File::with_name("default.config.toml"))
            .add_source(File::with_name(&format!("{mode}.config.toml")).required(false))
            .add_source(Environment::default())
            .build()?;
        config.try_deserialize()
    }
}