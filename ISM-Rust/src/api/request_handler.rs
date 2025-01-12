use std::sync::Arc;
use axum::{Extension, Json};
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum_keycloak_auth::decode::KeycloakToken;
use chrono::Utc;
use log::error;
use uuid::Uuid;
use crate::api::errors::{ErrorMessage, HttpError};
use crate::api::{AppState, NotificationCache};
use crate::database::{get_message_repository_instance, Message, NewMessage, UserRepository};

pub async fn poll_for_new_messages(
    Extension(token): Extension<KeycloakToken<String>>,
    Extension(notifications): Extension<NotificationCache>
) -> impl IntoResponse {
    let id = match Uuid::try_parse(&token.subject) {
        Ok(uuid) => uuid,
        Err(_) => {
            return HttpError::bad_request("Invalid token subject").into_response();
        }
    };
    let reader = notifications.read().await;
    if let Some(notes) = reader.get(&id){
        let notification = notes.read().await.clone();
        Json(notification).into_response()
    } else {
        Json::<Vec<String>>(vec![]).into_response()
    }
}

pub async fn scroll_chat_timeline() -> &'static str {
    "Not Implemented"
}

pub async fn send_message(
    Extension(token): Extension<KeycloakToken<String>>,
    Json(payload): Json<NewMessage>
) -> impl IntoResponse {
    let db = get_message_repository_instance().await;
    let id = match Uuid::try_parse(&token.subject) {
        Ok(uuid) => uuid,
        Err(_) => {
            return HttpError::bad_request("Invalid token subject").into_response();
        }
    };
    let msg = Message {
        chat_room_id: payload.chat_room_id,
        message_id: Uuid::new_v4(),
        sender_id: id,
        msg_body: payload.msg_body,
        msg_type: payload.msg_type.to_string(),
        created_at: Utc::now(),
    };
    match db.insert_data(msg.clone()).await {
        Ok(_) => {(StatusCode::CREATED, Json(msg)).into_response()},
        Err(err) => {
            error!("{}", err.to_string());
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}


pub async fn user_test(
    Path(user_id): Path<Uuid>,
    Extension(state): Extension<Arc<AppState>>
) -> impl IntoResponse {
    match state.user_repository.get_user(user_id).await {
        Ok(Some(user)) => Json(user).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "User not found").into_response(),
        Err(_) => (StatusCode::BAD_REQUEST, "Failed to fetch user").into_response()
    }
}

pub async fn get_me(
    Extension(token): Extension<KeycloakToken<String>>,
    Extension(state): Extension<Arc<AppState>>
) -> impl IntoResponse {
    let id = match Uuid::try_parse(&token.subject) {
        Ok(uuid) => uuid,
        Err(_) => {
            return HttpError::bad_request("Invalid token subject").into_response();
        }
    };
    match state.user_repository.get_user(id).await {
        Ok(Some(user)) => Json(user).into_response(),
        Ok(None) => HttpError::unauthorized(ErrorMessage::UserNoLongerExist.to_string()).into_response(),
        Err(_) => HttpError::bad_request(ErrorMessage::ServerError.to_string()).into_response()
    }
}