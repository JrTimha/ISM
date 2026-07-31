use crate::broadcast::{BroadcastChannel, Notification, NotificationEvent};
use crate::cache::util::ROOM_CONTEXT;
use crate::rooms::room_member::RoomContext;
use redis::aio::ConnectionManager;
use redis::{AsyncTypedCommands, PushInfo, RedisError, from_redis_value};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::mpsc::UnboundedReceiver;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

#[derive(Debug, Error)]
enum ProcessorError {
    #[error("Invalid push message structure")]
    InvalidPushFormat,

    #[error("Failed to deserialize payload: {0}")]
    PayloadDeser(#[from] serde_json::Error),

    #[error("Redis error: {0}")]
    Redis(#[from] RedisError),

    #[error("Redis parsing error: {0}")]
    RedisParsing(#[from] redis::ParsingError),
}

/// Forwards notifications published to the Redis chat keyspace onto local connections.
///
/// Takes the [`BroadcastChannel`] as an argument rather than reaching for a global. The task is
/// spawned by [`AppStateBuilder`](crate::core::AppStateBuilder), which is the only place where both
/// the Redis connection and the bus exist — the ordering that a global used to paper over.
pub async fn run_event_processor(mut rx: UnboundedReceiver<PushInfo>, mut conn: ConnectionManager, bus: Arc<BroadcastChannel>) {
    let _ = rx.recv().await;
    info!("Redis Event-Processing active.");

    while let Some(push_message) = rx.recv().await {
        debug!(?push_message, "Received push message");
        let notification = match parse_push_message(push_message) {
            Ok(message) => message,
            Err(error) => {
                warn!(?error, "Parsing of received push message failed, ignoring");
                continue;
            }
        };

        if let Err(e) = handle_notification(notification, &mut conn, &bus).await {
            error!(error = %e, "Failed to process notification");
        }
    }
}

fn parse_push_message(mut push_message: PushInfo) -> Result<Notification, ProcessorError> {
    let Some(payload_value) = push_message.data.pop() else {
        return Err(ProcessorError::InvalidPushFormat);
    };

    let payload_str: String = from_redis_value(payload_value)?;
    let notification: Notification = serde_json::from_str(&payload_str)?;

    Ok(notification)
}

async fn handle_notification(notification: Notification, conn: &mut ConnectionManager, bus: &BroadcastChannel) -> Result<(), ProcessorError> {
    if let NotificationEvent::ChatMessage { message, .. } = &notification.body {
        let key = format!("{}{}", ROOM_CONTEXT, message.chat_room_id);
        let json: Option<String> = conn.get(&key).await.unwrap_or(None);
        let member_ids: Vec<Uuid> = json
            .and_then(|s| serde_json::from_str::<RoomContext>(&s).ok())
            .map(|ctx| ctx.member_ids())
            .unwrap_or_default();
        if !member_ids.is_empty() {
            bus.send_event_to_all(member_ids, notification).await;
        }
    }
    Ok(())
}
