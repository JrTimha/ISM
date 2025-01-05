use axum::Router;
use axum::routing::get;

pub async fn init_router() -> Router {
    Router::new() //add new routes here
        .route("/", get(|| async { "Hello, World!" }))
}