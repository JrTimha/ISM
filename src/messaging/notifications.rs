use std::time::Duration;
use axum::{Extension, Json};
use axum::response::{IntoResponse, Sse};
use axum::response::sse::Event;
use futures::Stream;
use log::error;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use crate::broadcast::{BroadcastChannel};
use crate::keycloak::decode::KeycloakToken;


pub async fn stream_server_events(
    Extension(token): Extension<KeycloakToken<String>>
) -> Sse<impl Stream<Item = Result<Event, BroadcastStreamRecvError>>> {

    use futures::StreamExt;

    let receiver = BroadcastChannel::get().subscribe_to_user_events(token.subject.clone()).await;

    let stream = BroadcastStream::new(receiver).filter_map(move |notification| async move {
        match notification {
            Ok(event) => {
                let sse = Event::default().data(serde_json::to_string(&event).unwrap());
                Some(Ok(sse))
            }
            Err(error) => {
                error!("{}", error);
                None
            }
        }
    });
    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(5))
            .text("keep-alive-text")
    )
}

pub async fn get_latest_notification_events() -> impl IntoResponse {
    //todo: query latest events
    //placeholder
    Json::<Vec<String>>(vec![]).into_response()
}