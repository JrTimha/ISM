//! Messaging endpoints: sending a message, and the live notification stream.
//!
//! The stream handlers are the longest in the project, but what remains here is transport: axum's
//! SSE stream adapter, the WebSocket select loop, ping/pong. Everything a client could observe
//! about *which* events it gets — subscription, replay, deduplication, resync — belongs to
//! [`NotificationService`].

use crate::auth::CurrentUser;
use crate::broadcast::Notification;
use crate::core::errors::{AppError, AppResponse};
use crate::messaging::model::{MessageDto, NewMessage};
use crate::messaging::service::NotificationService;
use crate::messaging::{MessageService, service::ConnectionGuard};
use axum::Json;
use axum::extract::ws::{
    CloseFrame, Message, Utf8Bytes, WebSocket, WebSocketUpgrade, close_code,
};
use axum::extract::{Query, State};
use axum::response::sse::{Event, KeepAlive};
use axum::response::{IntoResponse, Sse};
use bytes::Bytes;
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tokio::time;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tracing::{debug, warn};
use uuid::Uuid;
use validator::Validate;

pub async fn handle_send_message(
    State(messages): State<MessageService>,
    user: CurrentUser,
    Json(payload): Json<NewMessage>,
) -> Result<Json<MessageDto>, AppError> {
    payload.validate().map_err(AppError::from)?;
    let response_msg = messages.send_message(payload, user.subject).await?;
    Ok(Json(response_msg))
}

/// Handshake parameters shared by the SSE and WebSocket endpoints. The client passes the
/// highest sequence number it has already seen; the server replays everything after it.
/// Omitted on a fresh connection (the client loads its initial state via REST instead).
#[derive(Deserialize)]
pub struct StreamHandshakeParams {
    #[serde(default)]
    last_seq: Option<u64>,
}

/// Build the live notification stream wire format.
fn notification_to_sse(notification: &Notification) -> Event {
    Event::default().data(serde_json::to_string(notification).unwrap_or_default())
}

pub async fn stream_server_events(
    user: CurrentUser,
    State(notifications): State<NotificationService>,
    Query(params): Query<StreamHandshakeParams>,
) -> Sse<impl Stream<Item = Result<Event, BroadcastStreamRecvError>>> {
    use futures::StreamExt;

    // Bound out of the token: the live stream below outlives this scope and only needs the id.
    let user_id = user.subject;

    // Subscribe before reading the replay so live events produced during the handshake are
    // buffered and not lost (subscribe-then-replay ordering).
    let receiver = notifications.subscribe(user_id).await;
    let guard: ConnectionGuard = notifications.connection_guard(user_id);

    let (replay, high_water) = notifications.resolve_handshake(&user_id, params.last_seq).await;

    let replay_stream = futures::stream::iter(replay.into_iter().map(|n| Ok(notification_to_sse(&n))));

    let live_stream = BroadcastStream::new(receiver).filter_map(move |result| {
        let _moved_guard = &guard; // tie the guard's lifetime to the live stream
        async move {
            match result {
                Ok(event) => {
                    // Ephemeral events (seq == None) always pass; durable events already
                    // covered by the replay window are dropped to avoid duplicates.
                    if event.seq.is_none_or(|s| s > high_water) {
                        Some(Ok(notification_to_sse(&event)))
                    } else {
                        None
                    }
                }
                Err(BroadcastStreamRecvError::Lagged(n)) => {
                    warn!(%user_id, lagged_events = n, "SSE client lagged, signalling resync");
                    Some(Ok(notification_to_sse(&NotificationService::resync("stream lagged, please resync via REST"))))
                }
            }
        }
    });

    // Ends the stream when the server starts shutting down. Without this the response body never
    // completes, and axum's graceful shutdown waits on this connection forever — see
    // `NotificationService::cancelled`. Ending the stream drops it, which drops the
    // `ConnectionGuard` tied into `live_stream` below, so the usual unsubscribe still runs.
    let stream = replay_stream
        .chain(live_stream)
        .take_until(notifications.cancelled());

    Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(5))
                .text("live-connection-heartbeat")
        )
}

pub async fn websocket_server_events(
    websocket: WebSocketUpgrade,
    user: CurrentUser,
    State(notifications): State<NotificationService>,
    Query(params): Query<StreamHandshakeParams>,
) -> impl IntoResponse {
    // Bound out of the token so the upgrade closure captures a `Copy` id, not the whole token.
    let user_id = user.subject;
    websocket
        .on_failed_upgrade(|error| warn!("Error upgrading websocket: {}", error))
        .on_upgrade(move |socket| handle_socket(socket, notifications, user_id, params.last_seq))
}

async fn handle_socket(mut socket: WebSocket, notifications: NotificationService, user_id: Uuid, last_seq: Option<u64>) {
    let mut broadcast_events = notifications.subscribe(user_id).await;
    let _guard = notifications.connection_guard(user_id);

    // Handshake: replay missing durable events (or send a resync signal) before going live.
    let (replay, mut high_water) = notifications.resolve_handshake(&user_id, last_seq).await;
    for notification in &replay {
        let json = serde_json::to_string(notification).unwrap_or_default();
        if socket.send(Message::text(json)).await.is_err() {
            debug!("Client disconnected during replay, closing.");
            return;
        }
    }

    let mut ping_interval = time::interval(Duration::from_secs(15));
    let mut last_pong_received = time::Instant::now();

    // Created once and pinned so the loop polls the same future every pass; rebuilding it inside
    // `select!` would restart the wait on each iteration and the signal could be missed.
    let cancelled = notifications.cancelled();
    tokio::pin!(cancelled);

    loop {
        tokio::select! {
            // 0. Server is shutting down — an upgraded socket is past the HTTP layer, so axum
            //    cannot close it for us. Say goodbye properly: 1001 tells the client this is a
            //    server going away, not an error, so it reconnects instead of backing off.
            _ = &mut cancelled => {
                debug!(%user_id, "Server shutting down, closing websocket");
                let _ = socket
                    .send(Message::Close(Some(CloseFrame {
                        code: close_code::AWAY,
                        reason: Utf8Bytes::from_static("server shutting down"),
                    })))
                    .await;
                break;
            }

            // 1. Handle new broadcasting event:
            notification_result = broadcast_events.recv() => {
                match notification_result {
                    Ok(event) => {
                        // Skip durable events already covered by the replay window.
                        if event.seq.is_some_and(|s| s <= high_water) {
                            continue;
                        }
                        if let Some(seq) = event.seq {
                            high_water = seq;
                        }
                        let json_msg = serde_json::to_string(&event).unwrap_or_default();
                        if socket.send(Message::text(json_msg)).await.is_err() {
                            debug!(%user_id, "Failed to send message to client, closing");
                            break;
                        }
                    }
                    Err(RecvError::Closed) => {
                        debug!("Client disconnected or channel closed");
                        break;
                    }
                    Err(RecvError::Lagged(n)) => {
                        warn!(%user_id, lagged_events = n, "WS client lagged, signalling resync");
                        let resync = serde_json::to_string(&NotificationService::resync("stream lagged, please resync via REST")).unwrap_or_default();
                        if socket.send(Message::text(resync)).await.is_err() {
                            break;
                        }
                        // The client will reload via REST, so stop deduplicating against the
                        // (now stale) high-water mark and forward everything going forward.
                        high_water = 0;
                    }
                }
            }

            // 2. Regular ping from ism:
            _ = ping_interval.tick() => {

                if last_pong_received.elapsed() > Duration::from_secs(30) {
                    debug!("Client did not respond to ping in time, closing websocket connection");
                    break;
                }

                if socket.send(Message::Ping(Bytes::new())).await.is_err() { // connection is dead when we can't send ping
                    break;
                }
            }

            // 3. Receive messages from the client:
            client_msg = socket.recv() => {
                match client_msg {
                    Some(Ok(Message::Close(_))) | None => {
                        debug!("Client has closed the websocket connection, closing.");
                        break;
                    }, //client is closing connection
                    Some(Err(_)) => {
                        debug!("Client has an error with the websocket connection, closing.");
                        break;
                    }, //client error
                    Some(Ok(Message::Pong(_))) => {
                        debug!("Client has sent Websocket-Pong");
                        last_pong_received = time::Instant::now();
                    }
                    Some(Ok(_)) => {
                        last_pong_received = time::Instant::now();
                    }
                }
            }
        }
    }
}

#[derive(Deserialize)]
pub struct NotificationQueryParam {
    last_seq: u64,
}

/// Current per-user sequence cursor. A client that has just completed a full REST sync reads this
/// to learn the sequence its snapshot corresponds to, then persists it as the baseline for future
/// short reconnects. The REST-sync itself opens its live stream **without** a `last_seq` parameter
/// (fresh connection, no replay) — this endpoint only seeds the stored cursor.
#[derive(Serialize)]
pub struct NotificationCursor {
    seq: u64,
}

pub async fn get_notification_cursor(State(notifications): State<NotificationService>, user: CurrentUser) -> AppResponse<Json<NotificationCursor>> {
    let seq = notifications.current_sequence(&user.subject).await?;
    Ok(Json(NotificationCursor { seq }))
}

pub async fn get_latest_notification_events(
    State(notifications): State<NotificationService>,
    user: CurrentUser,
    Query(params): Query<NotificationQueryParam>,
) -> AppResponse<Json<Vec<Notification>>> {
    let events = notifications.events_since(&user.subject, params.last_seq).await?;
    Ok(Json(events))
}
