use std::time::Duration;
use axum::{Extension, Json};
use axum::response::{IntoResponse, Sse};
use axum::response::sse::Event;
use futures::Stream;
use log::error;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use uuid::Uuid;
use crate::broadcast::{BroadcastChannel};
use crate::keycloak::decode::KeycloakToken;

struct ConnectionGuard {
    user_id: Uuid,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) { //triggering an unsubscribe, functions like a destructor
        let user_id = self.user_id.clone();
        tokio::spawn(async move {
            BroadcastChannel::get().unsubscribe(user_id).await;
        });
    }
}


pub async fn stream_server_events(
    Extension(token): Extension<KeycloakToken<String>>
) -> Sse<impl Stream<Item = Result<Event, BroadcastStreamRecvError>>> {

    use futures::StreamExt;

    let receiver = BroadcastChannel::get().subscribe_to_user_events(token.subject.clone()).await;
    let _guard = ConnectionGuard { user_id: token.subject.clone() };

    let stream = BroadcastStream::new(receiver).filter_map(move |notification| {

        let _moved_guard = &_guard; //lifetime of guard is extended to the stream and will end when the sse connection is closed

        async move {
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