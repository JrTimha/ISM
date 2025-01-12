use std::collections::HashMap;
use std::sync::Arc;
use axum::http::{HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json, Router};
use axum::body::Body;
use axum::extract::Path;
use axum::http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use axum::routing::{get, options, post};
use axum_keycloak_auth::{Url, instance::KeycloakConfig, instance::KeycloakAuthInstance, layer::KeycloakAuthLayer, PassthroughMode};
use axum_keycloak_auth::decode::KeycloakToken;
use chrono::Utc;
use log::{error};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use uuid::Uuid;
use tokio::sync::RwLock;
use crate::api::errors::{ErrorMessage, HttpError};
use crate::api::notification::Notification;
use crate::api::NotificationCache;
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
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS]);
    let notify_cache = init_notify_cache().await;

    let public_routing = Router::new()
        .route("/api/notify", options(handle_options))
        .route("/", get(|| async { "Hello, world! I'm your new ISM. 🤗" }));

    let protected_routing = Router::new() //add new routes here
        .route("/api/notify", get(poll_for_new_messages))
        .route("/api/timeline", get(scroll_chat_timeline))
        .route("/api/send-msg", post(send_message))
        .route("/api/users/{user_id}", get(user_test))
        .route("/api/users/get-me", get(get_me))
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .layer(init_auth(app_state.env.token_issuer.clone()))
        .layer(Extension(app_state))
        .layer(Extension(notify_cache));
    public_routing.merge(protected_routing)
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
        .persist_raw_claims(true)
        .expected_audiences(vec![String::from("account")])
        .build()
}

async fn handle_options() -> impl IntoResponse {
    Response::builder()
        .status(StatusCode::OK)
        .header("Access-Control-Allow-Origin", "http://localhost:4200")
        .header("Access-Control-Allow-Methods", "GET, OPTIONS, POST")
        .header("Access-Control-Allow-Headers", "Authorization, Content-Type, Accept")
        .header("Access-Control-Allow-Credentials", "true")
        .body(Body::empty())
        .unwrap()
}

async fn init_notify_cache() -> NotificationCache {
    let notifications: NotificationCache = Arc::new(RwLock::new(HashMap::new()));
    let cache_clone = Arc::clone(&notifications);
    tokio::spawn(async move {
        cleanup_old_notifications(cache_clone).await;
    });
    notifications
}

async fn cleanup_old_notifications(cache: NotificationCache) {
    loop {
        // 5 Minuten = 300 Sekunden
        let expiration_duration = chrono::Duration::seconds(10);
        let now = Utc::now();
        // Zugriff auf die gesamte HashMap
        let map = cache.read().await;
        for (user_id, notifications) in map.iter() {
            let mut user_notifications = notifications.write().await;
            // Entferne alte Notifications
            user_notifications.retain(|notification| {
                (now - notification.created_at) < expiration_duration
            });

            if user_notifications.is_empty() {
                println!("Notifications for user {user_id} have been cleared.");
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
    }
}


async fn poll_for_new_messages(
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