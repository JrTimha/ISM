use axum::Router;
use log::{info};
use ism::api::init_router;
use tokio::net::TcpListener;
use ism::core::ISMConfig;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let config = ISMConfig::new_config().unwrap_or_else(|err| panic!("Missing needed env: {}", err));

    let env = env_logger::Env::default().default_filter_or(config.log_level);
    env_logger::Builder::from_env(env).init();
    info!("Starting ISM...");

    let app: Router = init_router().await;
    let url = format!("{}:{}", config.ism_url, config.ism_port);
    let listener = TcpListener::bind(url.clone()).await.unwrap();
    info!("Server is listening on: {url}");
    axum::serve(listener, app).await.unwrap();
    info!("Stopping ISM...");
}