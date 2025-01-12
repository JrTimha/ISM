use std::collections::HashMap;
use std::sync::Arc;
use axum::http::{HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Router};
use axum::body::Body;
use axum::http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use axum::routing::{get, options, post};
use axum_keycloak_auth::{Url, instance::KeycloakConfig, instance::KeycloakAuthInstance, layer::KeycloakAuthLayer, PassthroughMode};
use chrono::Utc;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tokio::sync::RwLock;
use crate::api::NotificationCache;
use crate::api::request_handler::{get_me, poll_for_new_messages, scroll_chat_timeline, send_message, user_test};
use crate::core::{ISMConfig, TokenIssuer};
use crate::database::{UserDbClient};


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