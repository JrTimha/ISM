use std::sync::Arc;
use std::time::Duration;
use axum::{Extension, Json};
use axum::extract::{Query, State};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::{IntoResponse, Sse};
use axum::response::sse::Event;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::Stream;
use tokio::time;
use log::{debug, error};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tracing::warn;
use uuid::Uuid;
use crate::broadcast::{BroadcastChannel, Notification};
use crate::core::AppState;
use crate::errors::{AppError, AppResponse};
use crate::keycloak::decode::KeycloakToken;

struct ConnectionGuard {
    user_id: Uuid,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) { //triggering an unsubscribe, functions like a destructor
        let user_id = self.user_id.clone();
        tokio::spawn(async move {
            BroadcastChannel::get().unsubscribe(user_id).await;
        });
    }
}


pub async fn stream_server_events(
    Extension(token): Extension<KeycloakToken<String>>
) -> Sse<impl Stream<Item = Result<Event, BroadcastStreamRecvError>>> {

    use futures::StreamExt;

    let receiver = BroadcastChannel::get().subscribe_to_user_events(token.subject.clone()).await;
    let _guard = ConnectionGuard { user_id: token.subject.clone() };

    let stream = BroadcastStream::new(receiver).filter_map(move |notification| {

        let _moved_guard = &_guard; //lifetime of guard is extended to the stream and will end when the sse connection is closed

        async move {
            match notification {
                Ok(event) => {
                    let sse = Event::default().data(serde_json::to_string(&event).unwrap());
                    Some(Ok(sse))
                }
                Err(error) => {
                    error!("{}", error);
                    None
                }
            }
        }

    });
    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(5))
            .text("keep-alive-text")
    )
}


pub async fn websocket_server_events(
    websocket: WebSocketUpgrade,
    Extension(token): Extension<KeycloakToken<String>>
) -> impl IntoResponse {

    websocket
        .on_failed_upgrade(|error| warn!("Error upgrading websocket: {}", error))
        .on_upgrade(move |socket| handle_socket(socket, token.subject.clone()))
}

async fn handle_socket(mut socket: WebSocket, user_id: Uuid) {

    let mut broadcast_events = BroadcastChannel::get().subscribe_to_user_events(user_id.clone()).await;
    let _guard = ConnectionGuard { user_id };
    let mut ping_interval = time::interval(Duration::from_secs(15));
    let mut last_pong_received = time::Instant::now();

    loop {
        tokio::select! {
            // 1. Handle new broadcasting event:
            notification_result = broadcast_events.recv() => {
                match notification_result {
                    Ok(event) => {
                        let json_msg = serde_json::to_string(&event).unwrap();
                        let ws_message = Message::text(json_msg);

                        if socket.send(ws_message).await.is_err() {
                            error!("Failed to send message to client");
                        }
                    }
                    Err(RecvError::Closed) => {
                        debug!("Client disconnected or channel closed");
                        break;
                    }
                    Err(RecvError::Lagged(_)) => {
                        debug!("Client is too slow!")
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
    timestamp: DateTime<Utc>
}

pub async fn get_latest_notification_events(
    State(state): State<Arc<AppState>>,
    Extension(token): Extension<KeycloakToken<String>>,
    Query(params): Query<NotificationQueryParam>
) -> AppResponse<Json<Vec<Notification>>> {

    let notifications = state.cache.get_notifications_for_user(&token.subject, params.timestamp).await.map_err(|_| {
        AppError::ProcessingError("Error getting notifications: Cache Error".to_string())
    })?;
    Ok(Json(notifications))
}