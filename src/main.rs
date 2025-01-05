use axum::Router;
use log::{info};
use ism::api::init_router;
use tokio::net::TcpListener;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let env = env_logger::Env::default().default_filter_or("info");
    env_logger::Builder::from_env(env).init();
    info!("Starting ISM...");
    let app: Router = init_router().await;
    let listener = TcpListener::bind("0.0.0.0:7800").await.unwrap();
    info!("Server is listening on: 0.0.0.0:7800");
    axum::serve(listener, app).await.unwrap();
    info!("Stopping ISM...");
}
