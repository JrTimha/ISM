use axum::http::{HeaderValue, Method, StatusCode};
use axum::response::IntoResponse;
use axum::{Json, Router};
use axum::http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use axum::routing::get;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use crate::database::get_message_repository_instance;

/**
 * Initializing the api routes.
 */
pub async fn init_router() -> Router {
    let cors = CorsLayer::new()
        .allow_origin("http://localhost:4200".parse::<HeaderValue>().unwrap())
        .allow_headers([AUTHORIZATION, ACCEPT, CONTENT_TYPE])
        .allow_credentials(true)
        .allow_methods([Method::GET, Method::POST]);


    Router::new() //add new routes here
        .route("/hello-world", get(|| async { "Hello, World!" }))
        .route("/notify", get(poll_for_new_messages))
        .route("/timeline", get(scroll_chat_timeline))
        .route("/send-msg", get(send_message))
        .layer(TraceLayer::new_for_http())
        .layer(cors)
}

async fn poll_for_new_messages() -> impl IntoResponse {
    let db = get_message_repository_instance().await;
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

async fn send_message() -> &'static str {
    "Not Implemented"
}