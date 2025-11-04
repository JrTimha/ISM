use std::sync::Arc;
use axum::{Extension, Json};
use axum::extract::State;
use crate::core::AppState;
use crate::errors::AppError;
use crate::keycloak::decode::KeycloakToken;
use crate::messaging::message_service::MessageService;
use crate::messaging::model::{MessageDTO, NewMessage};

pub async fn handle_send_message(
    State(state): State<Arc<AppState>>,
    Extension(token): Extension<KeycloakToken<String>>,
    Json(payload): Json<NewMessage>
) -> Result<Json<MessageDTO>, AppError> {

    let response_msg = MessageService::send_message(state, payload, token.subject).await?;
    Ok(Json(response_msg))
}