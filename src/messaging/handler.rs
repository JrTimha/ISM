use crate::auth::CurrentUser;
use crate::core::AppState;
use crate::core::errors::AppError;
use crate::messaging::message_service::MessageService;
use crate::messaging::model::{MessageDto, NewMessage};
use axum::Json;
use axum::extract::State;
use std::sync::Arc;
use validator::Validate;

pub async fn handle_send_message(
    State(state): State<Arc<AppState>>,
    user: CurrentUser,
    Json(payload): Json<NewMessage>,
) -> Result<Json<MessageDto>, AppError> {
    payload.validate().map_err(AppError::from)?;
    let response_msg = MessageService::send_message(state, payload, user.subject).await?;
    Ok(Json(response_msg))
}
