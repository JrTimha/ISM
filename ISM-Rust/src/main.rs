use std::env;
use std::sync::Arc;
use dotenv::dotenv;
use log::{info};
use tokio::net::TcpListener;
use ism::core::ISMConfig;
use ism::api::{init_router, AppState};
use ism::database::{init_message_db, init_room_db};
use tracing_subscriber::filter::LevelFilter;


//learn it here: https://github.com/AarambhDevHub/rust-backend-axum
#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let run_mode = env::var("ISM_MODE").unwrap_or_else(|_| "development".into());
    dotenv().ok();

    let config = ISMConfig::new_config(&run_mode).unwrap_or_else(|err| panic!("Missing needed env: {}", err));
    tracing_subscriber::fmt()
        .with_max_level(LevelFilter::DEBUG)
        .init();

    info!("Starting ISM in {run_mode} mode.");
    //init both database connections, exit application if failing
    init_message_db(&config.message_db_config).await;
    let user_db = init_room_db(&config.user_db_config).await;

    let app_state = AppState {
        env: config.clone(),
        social_repository: user_db,
    };

    //init api router:
    let app = init_router(Arc::new(app_state.clone())).await;
    let url = format!("{}:{}", config.ism_url, config.ism_port);
    let listener = TcpListener::bind(url.clone()).await.unwrap();
    info!("ISM-Server up and is listening on: http://{url}");
    axum::serve(listener, app).await.unwrap();
    info!("Stopping ISM...");
}