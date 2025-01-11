use std::sync::Arc;
use axum::http::{HeaderValue, Method, StatusCode};
use axum::response::IntoResponse;
use axum::{Extension, Json, Router};
use axum::extract::Path;
use axum::http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use axum::routing::{get, post};
use axum_keycloak_auth::{Url, instance::KeycloakConfig, instance::KeycloakAuthInstance, layer::KeycloakAuthLayer, PassthroughMode};
use axum_keycloak_auth::decode::KeycloakToken;
use chrono::Utc;
use log::{error};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use uuid::Uuid;
use crate::api::errors::{ErrorMessage, HttpError};
use crate::core::{ISMConfig, TokenIssuer};
use crate::database::{get_message_repository_instance, Message, NewMessage, UserDbClient, UserRepository};

#[derive(Debug, Clone)]
pub struct AppState {
    pub env: ISMConfig,
    pub user_repository: UserDbClient,
}

/**
 * Initializing the api routes.
 */
pub async fn init_router(app_state: Arc<AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(app_state.env.cors_origin.parse::<HeaderValue>().unwrap())
        .allow_headers([AUTHORIZATION, ACCEPT, CONTENT_TYPE])
        .allow_credentials(true)
        .allow_methods([Method::GET, Method::POST]);

    Router::new() //add new routes here
        .route("/", get(|| async { "Hello, world! I'm your new ISM. 🤗" }))
        .route("/notify", get(poll_for_new_messages))
        .route("/timeline", get(scroll_chat_timeline))
        .route("/send-msg", post(send_message))
        .route("/users/{user_id}", get(user_test))
        .route("/users/get-me", get(get_me))
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .layer(init_auth(app_state.env.token_issuer.clone()))
        .layer(Extension(app_state))
}

fn init_auth(config: TokenIssuer) -> KeycloakAuthLayer<String> {
    let keycloak_auth_instance = KeycloakAuthInstance::new(
        KeycloakConfig::builder()
            .server(Url::parse(&config.iss_host).unwrap())
            .realm(config.iss_realm)
            .build(),
    );
    KeycloakAuthLayer::<String>::builder()
        .instance(keycloak_auth_instance)
        .passthrough_mode(PassthroughMode::Block)
        .persist_raw_claims(false)
        .expected_audiences(vec![String::from("account")])
        .build()
}


async fn poll_for_new_messages(
    Extension(token): Extension<KeycloakToken<String>>,
    Extension(_state): Extension<Arc<AppState>>
) -> impl IntoResponse {
    let db = get_message_repository_instance().await;
    let _id = match Uuid::try_parse(&token.subject) {
        Ok(uuid) => uuid,
        Err(_) => {
            return HttpError::bad_request("Invalid token subject").into_response();
        }
    };
    match db.fetch_data().await {
        Ok(messages) => {
            Json(messages).into_response()
        }
        Err(_) => {
            (StatusCode::BAD_REQUEST, "Failed to fetch messages").into_response()
        }
    }
}

async fn scroll_chat_timeline() -> &'static str {
    "Not Implemented"
}

async fn send_message(
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
        msg_type: payload.msg_type,
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

async fn user_test(
    Path(user_id): Path<Uuid>,
    Extension(state): Extension<Arc<AppState>>
) -> impl IntoResponse {
    match state.user_repository.get_user(user_id).await {
        Ok(Some(user)) => Json(user).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "User not found").into_response(),
        Err(_) => (StatusCode::BAD_REQUEST, "Failed to fetch user").into_response()
    }
}

async fn get_me(
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