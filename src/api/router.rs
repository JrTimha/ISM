use axum::Router;
use axum::routing::get;
use crate::database::get_db_instance;

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

async fn poll_for_new_messages() -> String {
    let db = get_db_instance().await;
    let msgs = db.fetch_data().await;
    msgs.unwrap()
}

async fn scroll_chat_timeline() -> &'static str {
    "Not Implemented"
}

async fn send_message() -> &'static str {
    "Not Implemented"
}