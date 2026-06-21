use std::sync::Arc;
use std::time::Duration;
use axum::{Extension, Json};
use axum::extract::{Query, State};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::{IntoResponse, Sse};
use axum::response::sse::Event;
use bytes::Bytes;
use futures::Stream;
use tokio::time;
use log::{debug, error};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tracing::warn;
use uuid::Uuid;
use crate::broadcast::{BroadcastChannel, Notification, NotificationEvent};
use crate::cache::redis_cache::ReplayResult;
use crate::core::AppState;
use crate::core::errors::AppResponse;
use crate::auth::decode::KeycloakToken;

/// Handshake parameters shared by the SSE and WebSocket endpoints. The client passes the
/// highest sequence number it has already seen; the server replays everything after it.
/// Omitted on a fresh connection (the client loads its initial state via REST instead).
#[derive(Deserialize)]
pub struct StreamHandshakeParams {
    #[serde(default)]
    last_seq: Option<u64>,
}

struct ConnectionGuard {
    user_id: Uuid,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) { //triggering an unsubscribe, functions like a destructor
        let user_id = self.user_id;
        tokio::spawn(async move {
            BroadcastChannel::get().unsubscribe(user_id).await;
        });
    }
}

/// Build the live notification stream wire format.
fn notification_to_sse(notification: &Notification) -> Event {
    Event::default().data(serde_json::to_string(notification).unwrap_or_default())
}

/// Control notification telling the client its cached history is unavailable and it must
/// re-fetch authoritative state via REST.
fn resync_notification(reason: &str) -> Notification {
    Notification::new(NotificationEvent::Resync { reason: reason.to_string() })
}

/// Resolve the connection handshake into (events to replay first, high-water sequence).
///
/// The high-water sequence is the largest sequence the client is guaranteed to have after the
/// replay; live events with a sequence `<= high_water` are duplicates and get filtered out.
/// A returned `Resync` event sets the high-water back to 0 so the client receives every
/// subsequent live event while it reloads state out-of-band.
async fn resolve_handshake(
    bc: &BroadcastChannel,
    user_id: &Uuid,
    last_seq: Option<u64>,
) -> (Vec<Notification>, u64) {
    let last_seq = match last_seq {
        Some(seq) => seq,
        None => return (vec![], 0), // fresh connection: nothing to replay
    };

    match bc.replay_since(user_id, last_seq).await {
        Ok(ReplayResult::Events(events)) => {
            let high_water = events.iter().filter_map(|n| n.seq).max().unwrap_or(last_seq);
            (events, high_water)
        }
        Ok(ReplayResult::ResyncNeeded) => {
            (vec![resync_notification("history unavailable, please resync via REST")], 0)
        }
        Err(err) => {
            error!("Failed to fetch replay for {}: {}", user_id, err);
            (vec![resync_notification("replay error, please resync via REST")], 0)
        }
    }
}

pub async fn stream_server_events(
    Extension(token): Extension<KeycloakToken<String>>,
    Query(params): Query<StreamHandshakeParams>,
) -> Sse<impl Stream<Item = Result<Event, BroadcastStreamRecvError>>> {

    use futures::StreamExt;

    let user_id = token.subject;
    let bc = BroadcastChannel::get();

    // Subscribe before reading the replay so live events produced during the handshake are
    // buffered and not lost (subscribe-then-replay ordering).
    let receiver = bc.subscribe_to_user_events(user_id).await;
    let guard = ConnectionGuard { user_id };

    let (replay, high_water) = resolve_handshake(bc, &user_id, params.last_seq).await;

    let replay_stream = futures::stream::iter(
        replay.into_iter().map(|n| Ok(notification_to_sse(&n)))
    );

    let live_stream = BroadcastStream::new(receiver).filter_map(move |result| {
        let _moved_guard = &guard; // tie the guard's lifetime to the live stream
        async move {
            match result {
                Ok(event) => {
                    // Ephemeral events (seq == None) always pass; durable events already
                    // covered by the replay window are dropped to avoid duplicates.
                    if event.seq.map_or(true, |s| s > high_water) {
                        Some(Ok(notification_to_sse(&event)))
                    } else {
                        None
                    }
                }
                Err(BroadcastStreamRecvError::Lagged(n)) => {
                    warn!("SSE client {} lagged by {} events, signalling resync", user_id, n);
                    Some(Ok(notification_to_sse(&resync_notification("stream lagged, please resync via REST"))))
                }
            }
        }
    });

    let stream = replay_stream.chain(live_stream);

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(5))
            .text("keep-alive-text")
    )
}


pub async fn websocket_server_events(
    websocket: WebSocketUpgrade,
    Extension(token): Extension<KeycloakToken<String>>,
    Query(params): Query<StreamHandshakeParams>,
) -> impl IntoResponse {

    websocket
        .on_failed_upgrade(|error| warn!("Error upgrading websocket: {}", error))
        .on_upgrade(move |socket| handle_socket(socket, token.subject, params.last_seq))
}

async fn handle_socket(mut socket: WebSocket, user_id: Uuid, last_seq: Option<u64>) {

    let bc = BroadcastChannel::get();
    let mut broadcast_events = bc.subscribe_to_user_events(user_id).await;
    let _guard = ConnectionGuard { user_id };

    // Handshake: replay missing durable events (or send a resync signal) before going live.
    let (replay, mut high_water) = resolve_handshake(bc, &user_id, last_seq).await;
    for notification in &replay {
        let json = serde_json::to_string(notification).unwrap_or_default();
        if socket.send(Message::text(json)).await.is_err() {
            debug!("Client disconnected during replay, closing.");
            return;
        }
    }

    let mut ping_interval = time::interval(Duration::from_secs(15));
    let mut last_pong_received = time::Instant::now();

    loop {
        tokio::select! {
            // 1. Handle new broadcasting event:
            notification_result = broadcast_events.recv() => {
                match notification_result {
                    Ok(event) => {
                        // Skip durable events already covered by the replay window.
                        if event.seq.map_or(false, |s| s <= high_water) {
                            continue;
                        }
                        if let Some(seq) = event.seq {
                            high_water = seq;
                        }
                        let json_msg = serde_json::to_string(&event).unwrap_or_default();
                        if socket.send(Message::text(json_msg)).await.is_err() {
                            error!("Failed to send message to client, closing.");
                            break;
                        }
                    }
                    Err(RecvError::Closed) => {
                        debug!("Client disconnected or channel closed");
                        break;
                    }
                    Err(RecvError::Lagged(n)) => {
                        warn!("WS client {} lagged by {} events, signalling resync", user_id, n);
                        let resync = serde_json::to_string(&resync_notification("stream lagged, please resync via REST")).unwrap_or_default();
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
    last_seq: u64
}

pub async fn get_latest_notification_events(
    State(state): State<Arc<AppState>>,
    Extension(token): Extension<KeycloakToken<String>>,
    Query(params): Query<NotificationQueryParam>
) -> AppResponse<Json<Vec<Notification>>> {
    let notifications = match state.cache.get_notifications_since_seq(&token.subject, params.last_seq).await? {
        ReplayResult::Events(events) => events,
        ReplayResult::ResyncNeeded => vec![resync_notification("history unavailable, please resync via REST")],
    };
    Ok(Json(notifications))
}
