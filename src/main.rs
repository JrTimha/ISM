use std::env;
use dotenv::dotenv;
use log::{info};
use tokio::net::TcpListener;
use tokio::task;
use ism::core::{AppState, ISMConfig};
use ism::api::{init_router};
use ism::database::{MessageRepository, RoomDatabaseClient};
use tracing_subscriber::filter::LevelFilter;
use ism::broadcast::BroadcastChannel;
use ism::kafka::start_consumer;

//learn to code rust axum here:
//https://gitlab.com/famedly/conduit/-/tree/next?ref_type=heads
//https://github.com/AarambhDevHub/rust-backend-axum
#[tokio::main(flavor = "multi_thread")]
async fn main() {
    dotenv().ok();
    let run_mode = env::var("ISM_MODE").unwrap_or_else(|_| "development".into());
    let config = ISMConfig::new(&run_mode).unwrap_or_else(|err| panic!("Missing needed env: {}", err));
    tracing_subscriber::fmt()
        .with_max_level(LevelFilter::DEBUG)
        .init();

    info!("Starting up ISM in {run_mode} mode.");

    //init broadcaster channel
    BroadcastChannel::init().await;

    //init both database connections, exit application if failing
    let message_repository = MessageRepository::new(&config.message_db_config).await.unwrap_or_else(|err|{
        panic!("Failed to initialize message repository: {}", err);
    });

    let app_state = AppState {
        env: config.clone(),
        room_repository: RoomDatabaseClient::new(&config.user_db_config).await,
        message_repository
    };

    if app_state.env.use_kafka == true {
        let kafka_config = app_state.env.kafka_config.clone();
        task::spawn(async move {
            start_consumer(kafka_config).await;
        });
    }

    //init api router:
    let app = init_router(app_state.clone()).await;
    let url = format!("{}:{}", config.ism_url, config.ism_port);
    let listener = TcpListener::bind(url.clone()).await.unwrap();
    info!("ISM-Server up and is listening on: http://{url}");
    axum::serve(listener, app).await.unwrap();
    info!("Stopping ISM...");
}