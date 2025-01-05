use std::env;
use config::{Config, ConfigError, Environment, File};
use serde::Deserialize;


#[derive(Deserialize)]
#[allow(unused)]
pub struct ISMConfig {
    pub ism_port: u16,
    pub db_url: String,
    pub ism_url: String,
    pub log_level: String,
}


impl ISMConfig {
    pub fn new_config() -> Result<Self, ConfigError> {
        let run_mode = env::var("RUN_MODE").unwrap_or_else(|_| "development".into());
        let config = Config::builder()
            .add_source(File::with_name("default.config.toml"))
            .add_source(File::with_name(&format!("{run_mode}.config.toml")))
            .add_source(Environment::default())
            .build()?;
        config.try_deserialize()
    }
}