use axum::Router;
use axum::routing::get;

/**
 * Initializing the api routes.
 */
pub async fn init_router() -> Router {
    Router::new() //add new routes here
        .route("/hello-world", get(|| async { "Hello, World!" }))
        .route("/notify", get(poll_for_new_messages))
        .route("/timeline", get(scroll_chat_timeline))
        .route("/send-msg", get(send_message))
}

async fn poll_for_new_messages() -> &'static str {
    "Not Implemented"
}

async fn scroll_chat_timeline() -> &'static str {
    "Not Implemented"
}

async fn send_message() -> &'static str {
    "Not Implemented"
}