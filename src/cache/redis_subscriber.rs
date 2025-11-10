use log::info;
use redis::{PushInfo, from_redis_value, AsyncTypedCommands, RedisError};
use redis::aio::ConnectionManager;
use tokio::sync::mpsc::UnboundedReceiver;
use tracing::{error, warn};
use uuid::Uuid;
use crate::broadcast::{BroadcastChannel, Notification, NotificationEvent};
use thiserror::Error;

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
    // `let-else` flacht die `if let`-Pyramide elegant ab.
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
            let room_key = format!("room_members:{}", message.chat_room_id);
            let member_ids: Vec<Uuid> = match conn.smembers(&room_key).await {
                Ok(ids) => ids.into_iter().filter_map(|id_str| Uuid::parse_str(&id_str).ok()).collect(),
                Err(e) => {
                    error!("Fehler beim Abrufen von Raum-Mitgliedern: {}", e);
                    return Ok(())
                }
            };
            BroadcastChannel::get().send_event_to_all(member_ids, notification).await;
        }
        _ => {}
    }
    Ok(())
}
