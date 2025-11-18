use serde::Serialize;
use uuid::Uuid;
use crate::broadcast::Notification;

#[derive(Serialize)]
pub struct PushNotification {
    pub to_user: Vec<Uuid>,
    pub notification: Notification
}