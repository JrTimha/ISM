use ism::core::{AppState, ISMConfig};
use ism::router::init_router;
use ism::welcome::welcome;
use std::env;
use tokio::net::TcpListener;
use tokio::signal;
use tracing::info;
use tracing_subscriber::EnvFilter;

//learn to code rust axum here:
//https://gitlab.com/famedly/conduit/-/tree/next?ref_type=heads
//https://github.com/AarambhDevHub/rust-backend-axum
//https://github.com/rust-lang/crates.io/ <---- THE BEST!
#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let config = init_configuration();
    welcome();
    //init the app state including object_storage connections, broadcast channels, kafka etc.
    let app_state = AppState::new(config.clone()).await;

    //init api router:
    let app = init_router(app_state).await;
    let url = format!("{}:{}", config.ism_url, config.ism_port);
    let listener = TcpListener::bind(url.clone()).await.unwrap_or_else(|err| {
        panic!(
            "Unable to start TCP-Listener at URL: {}, error is: {}",
            url, err
        )
    });

    info!("ISM-Server up and is listening on: {url}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal()) //only working when there aren't active connections
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
    let run_mode = env::var("ISM_MODE").unwrap_or_else(|_| "development".into());
    let config =
        ISMConfig::new(&run_mode).unwrap_or_else(|err| panic!("Missing needed env: {}", err));

    init_tracing(&config.log_level);

    config
}

/// Installs the tracing subscriber.
///
/// `configured` is a full `EnvFilter` directive list, not a single level: a global default
/// followed by any number of `target=level` overrides, e.g. `"info,sqlx=warn,ism::auth=debug"`.
/// Targets are module paths, so they can be narrowed as far as needed.
///
/// `ISM_LOG_LEVEL` replaces the configured value entirely when set and non-empty.
fn init_tracing(configured: &str) {
    let directives = env::var("ISM_LOG_LEVEL")
        .ok()
        .filter(|it| !it.trim().is_empty())
        .unwrap_or_else(|| configured.to_owned());

    // Parse strictly and fail at startup. A typo previously fell back to a bare INFO default,
    // silently discarding every per-target override the operator had configured.
    let filter = EnvFilter::builder()
        .parse(&directives)
        .unwrap_or_else(|err| panic!("Invalid log filter {directives:?}: {err}"));

    tracing_subscriber::fmt().with_env_filter(filter).init();
}
