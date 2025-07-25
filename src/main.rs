use std::env;
use dotenv::dotenv;
use tokio::net::TcpListener;
use tokio::{signal, task};
use tracing::info;
use tracing_subscriber::EnvFilter;
use ism::core::{AppState, ISMConfig};
use ism::api::{init_router};
use ism::database::{MessageDatabase, ObjectDatabase, RoomDatabase};
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

    let filter = EnvFilter::try_from_env("ISM_LOG_LEVEL").unwrap()
        .add_directive(LevelFilter::INFO.into())
        .add_directive("scylla=info".parse().unwrap());

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .init();

    info!("Starting up ISM in {run_mode} mode.");
    //init broadcaster channel
    BroadcastChannel::init().await;
    
    //init app state and both database connections, exit application if failing
    let app_state = AppState {
        env: config.clone(),
        room_repository: RoomDatabase::new(&config.user_db_config).await,
        message_repository: MessageDatabase::new(&config.message_db_config).await,
        s3_bucket: ObjectDatabase::new(&config.object_db_config).await
    };

    if app_state.env.use_kafka == true {
        let kafka_config = app_state.env.kafka_config.clone();
        task::spawn(async move {
            start_consumer(kafka_config).await;
        });
    }

    //init api router:
    let app = init_router(app_state).await;
    let url = format!("{}:{}", config.ism_url, config.ism_port);
    let listener = TcpListener::bind(url.clone()).await.unwrap();
    info!("ISM-Server up and is listening on: {url}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())//only working if there are no active connections
        .await
        .unwrap();
    info!("Stopping ISM...");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}