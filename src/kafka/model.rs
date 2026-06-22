use crate::broadcast::Notification;
use serde::Serialize;
use uuid::Uuid;

#[derive(Serialize)]
pub struct PushNotification {
    pub to_user: Vec<Uuid>,
    pub notification: Notification,
}
