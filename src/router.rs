use std::sync::Arc;
use axum::http::{HeaderValue, Method, StatusCode};
use axum::{Router};
use axum::extract::DefaultBodyLimit;
use axum::http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use axum::response::IntoResponse;
use axum::routing::{get};
use http::header::{CONNECTION, CONTENT_LENGTH, ORIGIN};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tower::ServiceBuilder;
use url::Url;
use crate::core::{AppState, TokenIssuer};
use crate::keycloak::instance::{KeycloakAuthInstance, KeycloakConfig};
use crate::keycloak::layer::KeycloakAuthLayer;
use crate::keycloak::PassthroughMode;
use crate::messaging::routes::create_messaging_routes;
use crate::rooms::routes::create_room_routes;
use crate::user_relationship::routes::create_user_routes;

/**
 * Initializing the api routes.
 */
pub async fn init_router(app_state: AppState) -> Router {
    let origin = app_state.env.cors_origin.clone();
    let cors = CorsLayer::new()
        .allow_origin(origin.parse::<HeaderValue>().unwrap())
        .allow_headers([AUTHORIZATION, ACCEPT, CONTENT_TYPE, CONTENT_LENGTH, CONNECTION, ORIGIN])
        .allow_credentials(true)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS]);

    let public_routing = Router::new()
        .route("/", get(|| async { "Hello, world! I'm your new ISM. 🤗" }))
        .route("/health", get(|| async { (StatusCode::OK, "Healthy").into_response() }));

    
    let protected_routing = Router::new() //add new routes here
        .merge(create_room_routes())
        .merge(create_user_routes())
        .merge(create_messaging_routes())
        
        //layering bottom to top middleware
        .layer(
            ServiceBuilder::new() //layering top to bottom middleware
                .layer(TraceLayer::new_for_http()) //1
                .layer(cors)//2
                .layer(init_auth(app_state.env.token_issuer.clone())) //3..
                .layer(DefaultBodyLimit::max(5 * 1024 * 1024)) //max 5mb files
        )
        .with_state(Arc::new(app_state));
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