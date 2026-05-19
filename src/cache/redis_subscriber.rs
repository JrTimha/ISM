use log::info;
use redis::{PushInfo, from_redis_value, AsyncTypedCommands, RedisError};
use redis::aio::ConnectionManager;
use tokio::sync::mpsc::UnboundedReceiver;
use tracing::{error, warn};
use uuid::Uuid;
use crate::broadcast::{BroadcastChannel, Notification, NotificationEvent};
use crate::cache::util::ROOM_CONTEXT;
use thiserror::Error;
use crate::rooms::room_member::RoomContext;

#[derive(Debug, Error)]
enum ProcessorError {

    #[error("Ungültige Push-Nachrichten-Struktur")]
    InvalidPushFormat,

    #[error("Deserialisierung der Nutzlast fehlgeschlagen: {0}")]
    PayloadDeser(#[from] serde_json::Error),

    #[error("Redis-Fehler: {0}")]
    Redis(#[from] RedisError),

    #[error("Redis-Fehler: {0}")]
    RedisParsing(#[from] redis::ParsingError),
}

pub async fn run_event_processor(mut rx: UnboundedReceiver<PushInfo>, mut conn: ConnectionManager) {

    let _ = rx.recv().await;
    info!("Redis Event-Processing active.");

    while let Some(push_message) = rx.recv().await {
        info!("Received push message: {:?}", push_message);
        let notification = match parse_push_message(push_message) {
            Ok(message) => message,
            Err(error) => {
                warn!("Parsing of received push message failed. Ignoring. Push message: {:?}", error);
                continue;
            }
        };

        if let Err(e) = handle_notification(notification, &mut conn).await {
            error!("Fehler bei der Verarbeitung der Notification: {}", e);
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

async fn handle_notification(
    notification: Notification,
    conn: &mut ConnectionManager,
) -> Result<(), ProcessorError> {
    match &notification.body {
        NotificationEvent::ChatMessage { message, .. } => {
            let key = format!("{}{}", ROOM_CONTEXT, message.chat_room_id);
            let json: Option<String> = conn.get(&key).await.unwrap_or(None);
            let member_ids: Vec<Uuid> = json
                .and_then(|s| serde_json::from_str::<RoomContext>(&s).ok())
                .map(|ctx| ctx.member_ids())
                .unwrap_or_default();
            if !member_ids.is_empty() {
                BroadcastChannel::get().send_event_to_all(member_ids, notification).await;
            }
        }
        _ => {}
    }
    Ok(())
}
