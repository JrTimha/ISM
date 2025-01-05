use log::{info};
use ism::api::init_router;

#[tokio::main]
async fn main() {
    let env = env_logger::Env::default().default_filter_or("info");
    env_logger::Builder::from_env(env).init();
    info!("Starting ISM...");
    let app = init_router().await;
    let listener = tokio::net::TcpListener::bind("0.0.0.0:7800").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
