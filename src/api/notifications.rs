use std::sync::Arc;
use std::time::Duration;
use axum::{Extension, Json};
use axum::extract::State;
use axum::response::{IntoResponse, Sse};
use axum::response::sse::Event;
use futures::Stream;
use http::StatusCode;
use log::error;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use crate::api::errors::HttpError;
use crate::api::utils::parse_uuid;
use crate::broadcast::{BroadcastChannel, NewNotification, Notification};
use crate::core::AppState;
use crate::keycloak::decode::KeycloakToken;


pub async fn stream_server_events(
    Extension(token): Extension<KeycloakToken<String>>
) -> Sse<impl Stream<Item = Result<Event, BroadcastStreamRecvError>>> {

    use futures::StreamExt;
    let id = parse_uuid(&token.subject).unwrap();

    let receiver = BroadcastChannel::get().subscribe_to_user_events(id.clone()).await;

    let stream = BroadcastStream::new(receiver).filter_map(move |x| async move {
        match x {
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
            .interval(Duration::from_secs(4))
            .text("keep-alive-text")
    )
}

//todo: query latest events
pub async fn poll_for_new_notifications() -> impl IntoResponse {
    //placeholder
    Json::<Vec<String>>(vec![]).into_response()
}


pub async fn add_notification(
    State(state): State<Arc<AppState>>,
    Extension(token): Extension<KeycloakToken<String>>,
    Json(payload): Json<NewNotification>,
) -> impl IntoResponse {

    let client = match state.env.token_issuer.valid_admin_client.clone() {
        Some(client) => client,
        None => {
            return HttpError::bad_request("A valid admin client is not provided.").into_response()
        }
    };

    if token.authorized_party != client {
        return HttpError::unauthorized("This client is not allowed to add a notification!").into_response()
    }

    let notification = Notification {
        notification_event: payload.event_type,
        body: payload.body,
        created_at: payload.created_at,
        display_value: None
    };
    BroadcastChannel::get().send_event(notification, &payload.to_user).await;
    StatusCode::OK.into_response()
}
