use std::sync::Arc;
use axum::http::{HeaderValue, Method, StatusCode};
use axum::{Extension, Router};
use axum::http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum_keycloak_auth::{Url, instance::KeycloakConfig, instance::KeycloakAuthInstance, layer::KeycloakAuthLayer, PassthroughMode};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tower::ServiceBuilder;
use crate::api::notification::{CacheService};
use crate::api::request_handler::{create_room, get_joined_rooms, get_room_with_details, get_users_in_room, poll_for_new_notifications, scroll_chat_timeline, send_message};
use crate::core::{ISMConfig, TokenIssuer};
use crate::database::{PgDbClient};


#[derive(Debug, Clone)]
pub struct AppState {
    pub env: ISMConfig,
    pub social_repository: PgDbClient,
}

/**
 * Initializing the api routes.
 */
pub async fn init_router(app_state: Arc<AppState>) -> Router {
    let origin = app_state.env.cors_origin.clone();
    let cors = CorsLayer::new()
        .allow_origin(origin.parse::<HeaderValue>().unwrap())
        .allow_headers([AUTHORIZATION, ACCEPT, CONTENT_TYPE])
        .allow_credentials(true)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS]);

    let notify_cache = Arc::new(CacheService::new());
    notify_cache.start_cleanup_task(300);

    let public_routing = Router::new()
        .route("/", get(|| async { "Hello, world! I'm your new ISM. 🤗" }))
        .route("/health", get(|| async { (StatusCode::OK, "Healthy").into_response() }));

    let protected_routing = Router::new() //add new routes here
        .route("/api/notify", get(poll_for_new_notifications))
        .route("/api/timeline", get(scroll_chat_timeline))
        .route("/api/send-msg", post(send_message))
        .route("/api/rooms/create-room", post(create_room))
        .route("/api/rooms/{room_id}/users", get(get_users_in_room))
        .route("/api/rooms/{room_id}/detailed", get(get_room_with_details))
        .route("/api/rooms/{room_id}/timeline", get(scroll_chat_timeline))
        .route("/api/rooms", get(get_joined_rooms))
        //layering bottom to top middleware
        .layer(
            ServiceBuilder::new() //layering top to bottom middleware
                .layer(TraceLayer::new_for_http()) //1
                .layer(cors)//2
                .layer(init_auth(app_state.env.token_issuer.clone())) //3..
                .layer(Extension(app_state))
                .layer(Extension(notify_cache))
        );
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