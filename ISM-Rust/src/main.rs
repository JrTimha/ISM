use std::env;
use axum::Router;
use log::{info};
use tokio::net::TcpListener;
use ism::core::ISMConfig;
use ism::api::init_router;
use ism::database::{init_message_db, init_user_db, UserDbClient};
use tracing_subscriber::filter::LevelFilter;

#[derive(Debug, Clone)]
pub struct AppState {
    pub env: ISMConfig,
    pub user_repository: UserDbClient,
}

//learn it here: https://github.com/AarambhDevHub/rust-backend-axum
#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let run_mode = env::var("ISM_MODE").unwrap_or_else(|_| "development".into());
    let config = ISMConfig::new_config(&run_mode).unwrap_or_else(|err| panic!("Missing needed env: {}", err));
    tracing_subscriber::fmt()
        .with_max_level(LevelFilter::DEBUG)
        .init();

    info!("Starting ISM in {run_mode} mode.");
    //init both database connections, exit application if failing
    init_message_db(&config).await;
    init_user_db(&config).await;

    //init api router:
    let app: Router = init_router().await;
    let url = format!("{}:{}", config.ism_url, config.ism_port);
    let listener = TcpListener::bind(url.clone()).await.unwrap();
    info!("ISM-Server up and is listening on: {url}");
    axum::serve(listener, app).await.unwrap();
    info!("Stopping ISM...");
}

//fn init_logging(log_level: &str) {
//    let env = env_logger::Env::default().default_filter_or(log_level);
//    env_logger::Builder::from_env(env).init();
//}