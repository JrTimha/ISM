use std::env;
use axum::Router;
use log::{info};
use tokio::net::TcpListener;
use ism::core::ISMConfig;
use ism::api::init_router;
use ism::database::CassandraDb;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let run_mode = env::var("ISM_MODE").unwrap_or_else(|_| "development".into());
    let config = ISMConfig::new_config(&run_mode).unwrap_or_else(|err| panic!("Missing needed env: {}", err));
    init_logging(&config.log_level);
    info!("Starting ISM in {run_mode} mode.");

    let db = CassandraDb::new(&config).await.unwrap_or_else(|err| panic!("Failed to init database: {}", err));


    let app: Router = init_router().await;
    let url = format!("{}:{}", config.ism_url, config.ism_port);
    let listener = TcpListener::bind(url.clone()).await.unwrap();
    info!("ISM-Server up and is listening on: {url}");
    axum::serve(listener, app).await.unwrap();
    info!("Stopping ISM...");
}

fn init_logging(log_level: &str) {
    let env = env_logger::Env::default().default_filter_or(log_level);
    env_logger::Builder::from_env(env).init();
}