use config::{Config, ConfigError, Environment, File};
use serde::Deserialize;

#[derive(Deserialize)]
#[allow(unused)]
pub struct ISMConfig {
    pub ism_port: u16,
    pub db_url: String,
    pub ism_url: String,
    pub log_level: String,
    pub db_user: String,
    pub db_password: String,
    pub db_keyspace: String
}

//examples: https://github.com/rust-cli/config-rs/blob/main/examples/hierarchical-env/settings.rs
impl ISMConfig {
    pub fn new_config(mode: &str) -> Result<Self, ConfigError> {
        //layering the different environment variables, default values first, overwritten by config files and env-vars
        let config = Config::builder()
            .add_source(File::with_name("default.config.toml"))
            .add_source(File::with_name(&format!("{mode}.config.toml")))
            .add_source(Environment::default())
            .build()?;
        config.try_deserialize()
    }
}