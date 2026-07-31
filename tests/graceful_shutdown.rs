//! Proves that a live SSE connection no longer blocks shutdown.
//!
//! This is the bug the `ShutdownSignal` exists for, reproduced against a real `axum::serve`. Both
//! tests build the same server twice — once with the signal wired into the stream and once
//! without — so the pair shows that the mechanism is load-bearing rather than decorative.
//!
//! Nothing here touches PostgreSQL, Redis or Keycloak: the question is purely whether
//! `axum::serve` can finish while a response body is still open.

use axum::Router;
use axum::response::IntoResponse;
use axum::response::sse::{Event, Sse};
use axum::routing::get;
use futures::StreamExt;
use ism::core::{ShutdownController, ShutdownSignal};
use std::convert::Infallible;
use std::future::IntoFuture;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// Long enough for a server that *can* stop to stop; short enough that a hang fails fast.
const SETTLE: Duration = Duration::from_secs(5);

/// A stream that never yields and never ends — the essence of an idle SSE subscriber.
fn forever() -> impl futures::Stream<Item = Result<Event, Infallible>> + Send + 'static {
    futures::stream::pending()
}

/// Starts a server whose `/sse` route holds a connection open forever.
///
/// `listens_for_shutdown` selects whether the stream reacts to the signal, which is the single
/// difference between the two tests below.
async fn spawn_server(
    signal: ShutdownSignal,
    listens_for_shutdown: bool,
) -> (SocketAddr, oneshot::Sender<()>, JoinHandle<std::io::Result<()>>) {
    let app = Router::new().route(
        "/sse",
        get(move || {
            let signal = signal.clone();
            async move {
                // Erased to `Response` because the two branches are different `Sse<_>` types.
                if listens_for_shutdown {
                    Sse::new(forever().take_until(signal.cancelled())).into_response()
                } else {
                    Sse::new(forever()).into_response()
                }
            }
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binding an ephemeral port cannot fail");
    let addr = listener
        .local_addr()
        .expect("a bound listener always has an address");

    let (stop_accepting, accepted_stop) = oneshot::channel();
    let server = tokio::spawn(
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = accepted_stop.await;
            })
            .into_future(),
    );

    (addr, stop_accepting, server)
}

/// Opens `/sse` and returns the response, which must be **held** — dropping it closes the
/// connection and the test would prove nothing.
async fn open_sse(addr: SocketAddr) -> reqwest::Response {
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("client builds")
        .get(format!("http://{addr}/sse"))
        .send()
        .await
        .expect("the SSE endpoint answers");

    assert!(
        response.status().is_success(),
        "expected the stream to open, got {}",
        response.status()
    );
    response
}

#[tokio::test]
async fn a_live_sse_connection_does_not_block_shutdown() {
    let controller = ShutdownController::new();
    let (addr, stop_accepting, server) = spawn_server(controller.signal(), true).await;

    let _open_stream = open_sse(addr).await;

    // Exactly what `Shutdown::begin_when` does: stop accepting, then tell live streams to finish.
    let _ = stop_accepting.send(());
    controller.trigger();

    tokio::time::timeout(SETTLE, server)
        .await
        .expect("serve must finish while an SSE client is still connected")
        .expect("the server task must not panic")
        .expect("serving must end without an error");
}

#[tokio::test]
async fn without_the_signal_the_same_connection_hangs_shutdown() {
    // The regression this guards against. Axum's graceful shutdown only asks HTTP/1 to finish the
    // in-flight response, and an SSE body never finishes on its own — so telling axum to stop is
    // not enough, and a stream that ignores the signal keeps the whole process alive.
    let controller = ShutdownController::new();
    let (addr, stop_accepting, server) = spawn_server(controller.signal(), false).await;

    let _open_stream = open_sse(addr).await;

    let _ = stop_accepting.send(());
    controller.trigger();

    assert!(
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .is_err(),
        "a stream that ignores the shutdown signal is expected to hang serve — if this now \
         finishes, axum gained the ability to end live connections itself and \
         `NotificationService::cancelled` may no longer be needed"
    );
}
