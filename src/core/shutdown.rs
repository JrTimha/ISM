//! The signal that tells long-lived work to stop.
//!
//! # Why this has to exist
//!
//! Axum cannot end a live connection for you. Its graceful shutdown stops the accept loop and calls
//! `graceful_shutdown()` on each connection, which for HTTP/1 means "finish the in-flight
//! response" — and an SSE body never finishes. An upgraded WebSocket is past the HTTP layer
//! entirely and is not affected at all. The serve future then waits for those connection tasks
//! forever, with no timeout to configure.
//!
//! So a handler that holds a connection open **must** listen for shutdown itself. That is what this
//! module provides: one signal, broadcast to every listener, so the reaction is written once per
//! transport rather than once per connection.
//!
//! # The capability split
//!
//! [`ShutdownController`] can fire the signal; [`ShutdownSignal`] can only observe it. Services
//! receive the listen-only half, so "stop the server" stays a decision of the composition root and
//! cannot be reached from a request handler.
//!
//! Built on [`tokio::sync::watch`] rather than `tokio_util::sync::CancellationToken`: the two are
//! equivalent here, and `tokio` is already a dependency.

use std::future::Future;
use tokio::sync::watch;

/// Fires the shutdown signal.
///
/// Held by [`Shutdown`](crate::core::Shutdown), never by a service. `Clone` so it can be moved into
/// the `'static` future that `axum::serve` takes; every clone fires the same signal.
#[derive(Clone)]
pub struct ShutdownController {
    tx: watch::Sender<bool>,
    signal: ShutdownSignal,
}

impl ShutdownController {
    pub fn new() -> Self {
        let (tx, rx) = watch::channel(false);
        Self {
            tx,
            signal: ShutdownSignal { rx },
        }
    }

    /// A listen-only handle, to hand to a service that owns long-lived work.
    pub fn signal(&self) -> ShutdownSignal {
        self.signal.clone()
    }

    /// Begins shutdown. Idempotent — firing twice is the same as firing once.
    pub fn trigger(&self) {
        // Fails only when every receiver has been dropped, which means nothing is left to tell.
        let _ = self.tx.send(true);
    }
}

impl Default for ShutdownController {
    fn default() -> Self {
        Self::new()
    }
}

/// Listen-only half of the shutdown signal, cloned into every service that owns long-lived work.
#[derive(Clone)]
pub struct ShutdownSignal {
    rx: watch::Receiver<bool>,
}

impl ShutdownSignal {
    /// Resolves once shutdown has begun — immediately if it already has.
    ///
    /// That "already has" case matters: a connection accepted during the shutdown window must close
    /// straight away rather than wait for an edge that has already passed.
    ///
    /// Returns an **owned** future rather than borrowing `self`, because the SSE stream built from
    /// it outlives the handler that created it and has to satisfy `Sse<impl Stream + 'static>`.
    ///
    /// The `use<>` bound is what makes that true: under Rust 2024's capture rules an `impl Trait`
    /// return type implicitly captures every lifetime in scope — including `&self` — which would
    /// tie the future to the borrow. `use<>` says it captures nothing, so it is genuinely
    /// `'static`.
    pub fn cancelled(&self) -> impl Future<Output = ()> + Send + use<> {
        let mut rx = self.rx.clone();
        async move {
            // An error means the controller is gone, which is as good as cancelled — there is
            // nobody left who could still fire it.
            let _ = rx.wait_for(|fired| *fired).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::timeout;

    /// Shorter than any real grace period, long enough to let a ready future resolve.
    const TICK: Duration = Duration::from_millis(50);

    #[tokio::test]
    async fn stays_pending_until_triggered() {
        let controller = ShutdownController::new();
        let cancelled = controller.signal().cancelled();

        assert!(
            timeout(TICK, cancelled).await.is_err(),
            "a live connection must not be told to close before shutdown begins"
        );
    }

    #[tokio::test]
    async fn resolves_after_trigger() {
        let controller = ShutdownController::new();
        let cancelled = controller.signal().cancelled();

        controller.trigger();

        timeout(TICK, cancelled)
            .await
            .expect("the signal must reach a listener that was already waiting");
    }

    #[tokio::test]
    async fn resolves_immediately_when_already_triggered() {
        // A connection accepted during the shutdown window: it asks *after* the edge, so waiting
        // for the next change would hang it until the process was killed.
        let controller = ShutdownController::new();
        controller.trigger();

        timeout(TICK, controller.signal().cancelled())
            .await
            .expect("a listener created after the trigger must see the signal at once");
    }

    #[tokio::test]
    async fn a_dropped_controller_counts_as_cancelled() {
        // Nothing can fire the signal any more, so a listener that kept waiting would never
        // finish — and would hold a connection open forever.
        let controller = ShutdownController::new();
        let cancelled = controller.signal().cancelled();
        drop(controller);

        timeout(TICK, cancelled)
            .await
            .expect("a signal whose controller is gone must resolve rather than hang");
    }
}
