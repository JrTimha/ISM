use ism::core::{AppStateBuilder, Bootstrap, ISMConfig};
use ism::router::init_router;
use ism::welcome::welcome;
use std::process::ExitCode;
use tokio::net::TcpListener;
use tokio::signal;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::ParseError;

//learn to code rust axum here:
//https://gitlab.com/famedly/conduit/-/tree/next?ref_type=heads
//https://github.com/AarambhDevHub/rust-backend-axum
//https://github.com/rust-lang/crates.io/ <---- THE BEST!
#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    // Config before tracing, because the filter directives come out of the config. Both can fail
    // before there is a subscriber to log through, so these two report to stderr.
    let config = match ISMConfig::new() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("ISM could not read its configuration: {error}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(error) = init_tracing(&config.log_level) {
        eprintln!("ISM could not install its log filter: {error}");
        return ExitCode::FAILURE;
    }

    welcome(&config.run_mode);

    match run(config).await {
        Ok(()) => {
            info!("Stopping ISM...");
            ExitCode::SUCCESS
        }
        Err(error) => {
            // A failed startup is almost always a misconfigured host or a dependency that has not
            // come up yet. Report it as a message and a non-zero exit, not as a panic backtrace.
            error!(error = %error, "ISM could not start");
            ExitCode::FAILURE
        }
    }
}

async fn run(config: ISMConfig) -> Result<(), Box<dyn std::error::Error>> {
    let url = format!("{}:{}", config.ism_url, config.ism_port);

    // Wires the database, cache, object storage, event bus and every service — see
    // `ism::core::AppStateBuilder`.
    let Bootstrap { state, shutdown } = AppStateBuilder::new(config).build().await?;

    // `state` is moved into the router and stays there until the server stops; `shutdown` is the
    // disjoint half that has to outlive it.
    let app = init_router(state).await;
    let listener = TcpListener::bind(&url).await?;

    info!("ISM-Server up and is listening on: {url}");
    let served = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal()) //only working when there aren't active connections
        .await;

    // Runs whether the server stopped cleanly or failed: a serve error would otherwise skip
    // closing the pool and leave PostgreSQL holding the backends until its keepalive reaps them.
    shutdown.run().await;
    served?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c().await.expect("failed to install Ctrl+C handler");
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

/// Installs the tracing subscriber.
/// `directives` is a full `EnvFilter` directive list, not a single level: a global default
/// followed by any number of `target=level` overrides, e.g. `"info,sqlx=warn,ism::auth=debug"`.
/// Targets are module paths, so they can be narrowed as far as needed.
///
/// `ISM_LOG_LEVEL` still overrides it — not by being read here, but because it is an ordinary
/// `ISM_*` variable that the config layer maps onto `log_level` like any other field.
fn init_tracing(directives: &str) -> Result<(), ParseError> {
    // Parse strictly and fail at startup. A typo previously fell back to a bare INFO default,
    // silently discarding every per-target override the operator had configured.
    let filter = EnvFilter::builder().parse(directives)?;
    tracing_subscriber::fmt().with_env_filter(filter).init();
    Ok(())
}
