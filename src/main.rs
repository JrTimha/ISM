use std::env;
use dotenv::dotenv;
use tokio::net::TcpListener;
use tokio::{signal};
use tracing::info;
use tracing_subscriber::EnvFilter;
use ism::core::{AppState, ISMConfig};
use ism::api::{init_router};
use tracing_subscriber::filter::LevelFilter;
use ism::broadcast::BroadcastChannel;

//learn to code rust axum here:
//https://gitlab.com/famedly/conduit/-/tree/next?ref_type=heads
//https://github.com/AarambhDevHub/rust-backend-axum
#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let config = init_configuration();

    //init broadcaster channel
    BroadcastChannel::init().await;
    
    //init the app state including database connections, kafka etc.
    let app_state = AppState::new(config.clone()).await;

    //init api router:
    let app = init_router(app_state).await;
    let url = format!("{}:{}", config.ism_url, config.ism_port);
    let listener = TcpListener::bind(url.clone()).await.unwrap();
    info!("ISM-Server up and is listening on: {url}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())//only working when there aren't active connections
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

fn init_configuration() -> ISMConfig {
    dotenv().ok();
    let run_mode = env::var("ISM_MODE").unwrap_or_else(|_| "development".into());
    let config = ISMConfig::new(&run_mode).unwrap_or_else(|err| panic!("Missing needed env: {}", err));

    let filter = EnvFilter::builder()
        .with_env_var("ISM_LOG_LEVEL")
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy()
        .add_directive("scylla=info".parse().unwrap());

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .init();

    info!("Starting up ISM in {run_mode} mode.");

    config
}