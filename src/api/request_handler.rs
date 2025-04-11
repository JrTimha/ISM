use std::sync::Arc;
use std::time::Duration;
use axum::{Extension, Json};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Sse};
use axum::response::sse::Event;
use chrono::{DateTime, Utc};
use futures::{Stream};
use log::{error};
use serde::Deserialize;
use uuid::Uuid;
use crate::api::errors::{HttpError};
use crate::database::{RoomRepository};
use crate::keycloak::decode::KeycloakToken;
use crate::model::{ChatRoomDTO, Message, NewMessage, NewRoom, RoomType};
use tokio_stream::wrappers::{errors::BroadcastStreamRecvError, BroadcastStream};
use crate::broadcast::{BroadcastChannel, NewNotification, Notification, NotificationEvent};
use crate::core::AppState;

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

#[derive(Deserialize)]
pub struct TimelineQuery {
    timestamp: DateTime<Utc>
}

pub async fn scroll_chat_timeline(
    Extension(token): Extension<KeycloakToken<String>>,
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<Uuid>,
    Query(params): Query<TimelineQuery>
) -> impl IntoResponse {
    let id = parse_uuid(&token.subject).unwrap();
    if let Err(err) = check_user_in_room(&state, &id, &room_id).await {
        return err.into_response();
    }
    match state.message_repository.fetch_data(params.timestamp, room_id).await {
        Ok(data) => {
            Json(data).into_response()
        },
        Err(err) => {
            error!("{}", err.to_string());
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

pub async fn add_notification(
    Extension(token): Extension<KeycloakToken<String>>,
    Json(payload): Json<NewNotification>
) -> impl IntoResponse {
    println!("{:#?}", token.roles);
    println!("{:#?}", payload);

    let test = Notification {
        notification_event: payload.event_type,
        body: payload.body,
        created_at: payload.created_at,
        display_value: None
    };

    BroadcastChannel::get().send_event(test, &payload.to_user).await;
    StatusCode::OK.into_response()
}

pub async fn send_message(
    Extension(token): Extension<KeycloakToken<String>>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<NewMessage>
) -> impl IntoResponse {
    let id = parse_uuid(&token.subject).unwrap();

    let mut users = match state.room_repository.select_room_participants_ids(&payload.chat_room_id).await {
        Ok(ids) => ids,
        Err(error) => {
            error!("{}", error.to_string());
            return StatusCode::BAD_REQUEST.into_response();
        }
    };
    if !users.contains(&id) {
        return HttpError::unauthorized("Room not found or access denied.").into_response();
    }

    users.retain(|&user| {
        user != id
    });

    let msg = Message {
        chat_room_id: payload.chat_room_id,
        message_id: Uuid::new_v4(),
        sender_id: id,
        msg_body: payload.msg_body,
        msg_type: payload.msg_type.to_string(),
        created_at: Utc::now(),
    };
    let json = match serde_json::to_value(&msg) {
        Ok(json) => json,
        Err(_) => return StatusCode::BAD_REQUEST.into_response()
    };

    if let Err(err) = state.message_repository.insert_data(msg.clone()).await {
        error!("{}", err.to_string());
        return HttpError::bad_request("Can't safe message in timeline").into_response();
    }
    let displayed = match state.room_repository.update_last_room_message(&payload.chat_room_id, &msg).await {
        Ok(displayed) => displayed,
        Err(error) => {
            error!("{}", error);
            return HttpError::bad_request("Can't update the state of the chat room.").into_response();
        }
    };
    if let Err(err) = state.room_repository.update_user_read_status(&payload.chat_room_id, &msg.sender_id).await {
        error!("{}", err);
        return HttpError::bad_request("Can't update user read status.").into_response();
    }

    let note = Notification {
        notification_event: NotificationEvent::ChatMessage,
        body: json,
        created_at: msg.created_at,
        display_value: Option::from(displayed)
    };
    BroadcastChannel::get().send_event_to_all(users, note).await;
    (StatusCode::CREATED, Json(msg)).into_response()
}

pub async fn get_users_in_room(
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<Uuid>
) -> impl IntoResponse {
    match state.room_repository.select_all_user_in_room(&room_id).await {
        Ok(users) => Json(users).into_response(),
        Err(err) => HttpError::bad_request(err.to_string()).into_response()
    }
}

pub async fn get_joined_rooms(
    State(state): State<Arc<AppState>>,
    Extension(token): Extension<KeycloakToken<String>>,
) -> impl IntoResponse {
    let id = parse_uuid(&token.subject).unwrap();
    match state.room_repository.get_joined_rooms(&id).await {
        Ok(rooms) => Json(rooms).into_response(),
        Err(err) => HttpError::bad_request(err.to_string()).into_response()
    }
}

pub async fn get_room_with_details(
    State(state): State<Arc<AppState>>,
    Extension(token): Extension<KeycloakToken<String>>,
    Path(room_id): Path<Uuid>
) -> impl IntoResponse {
    let id = parse_uuid(&token.subject).unwrap();
    if let Err(err) = check_user_in_room(&state, &id, &room_id).await {
        return err.into_response();
    }

    let res = tokio::try_join!( //executing 2 queries async
        state.room_repository.select_room(&room_id),
        state.room_repository.select_all_user_in_room(&room_id)
    );

    match res {
        Ok((room, user)) => {
            let room_details = ChatRoomDTO {
                id: room.id,
                room_type: room.room_type,
                room_name: room.room_name,
                created_at: room.created_at,
                users: user,
            };
            Json(room_details).into_response()
        }
        Err(err) => {
            HttpError::bad_request(err.to_string()).into_response()
        }
    }

}

pub async fn mark_room_as_read(
    State(state): State<Arc<AppState>>,
    Extension(token): Extension<KeycloakToken<String>>,
    Path(room_id): Path<Uuid>
) -> impl IntoResponse {
    let id = parse_uuid(&token.subject).unwrap();
    match state.room_repository.update_user_read_status(&room_id, &id).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(_) => {
            HttpError::bad_request("Can't update user read status.").into_response()
        }
    }
}


pub async fn create_room(
    Extension(token): Extension<KeycloakToken<String>>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<NewRoom>
) -> impl IntoResponse {
    let id = parse_uuid(&token.subject).unwrap();

    if !payload.invited_users.contains(&id) {
        return HttpError::bad_request("Sender ID is not in the list of invited users.".to_string()).into_response();
    }

    match payload.room_type {
        RoomType::Single => {
            if payload.invited_users.len() != 2 {
                return HttpError::bad_request("Personal rooms must have exactly two IDs (sender + one other).".to_string()).into_response();
            }
        }
        RoomType::Group => {
            if payload.invited_users.len() < 2 {
                return HttpError::bad_request("Groups must have more than one user.".to_string()).into_response();
            }
        }
    }

    let room_entity = match state.room_repository.insert_room(payload.clone()).await {
        Ok(room) => room,
        Err(error) => {
            error!("{}", error);
            return HttpError::bad_request("Unable to persist the room.").into_response()
        }
    };

    let mut users = payload.invited_users.clone();
    users.retain(|&user| user != id); //don't send to the creator, he will get it from the http response

    if room_entity.room_type == RoomType::Single {
        let other_user = match users.first() {
            Some(other_user) => other_user,
            None => return HttpError::bad_request("Can't find other user.").into_response(),
        };

        let res = tokio::try_join!( //executing 2 queries async
        state.room_repository.find_specific_joined_room(&room_entity.id, &id),
        state.room_repository.find_specific_joined_room(&room_entity.id, &other_user)
        );
        match res {
            Ok((room_creator, room_participator)) => {
                if let (Some(creator_dto), Some(participator_dto)) = (room_creator, room_participator) {
                    let json = match serde_json::to_value(&participator_dto) {
                        Ok(json) => json,
                        Err(_) => return StatusCode::BAD_REQUEST.into_response()
                    };
                    let note = Notification {
                        notification_event: NotificationEvent::NewRoom,
                        body: json,
                        created_at: Utc::now(),
                        display_value: None
                    };
                    BroadcastChannel::get().send_event(note, other_user).await;
                    Json(creator_dto).into_response()
                } else {
                    HttpError::bad_request("Room for participator is null.").into_response()
                }
            }
            Err(error) => {
                error!("{}", error);
                HttpError::bad_request("Can't find the room.").into_response()
            }
        }

    } else { //is group room

        let room = match state.room_repository.find_specific_joined_room(&room_entity.id, &id).await {
            Ok(Some(room)) => room,
            Ok(None) => return HttpError::bad_request("Room not found after creation.").into_response(),
            Err(error) => {
                error!("{}", error);
                return HttpError::bad_request("Room not found after creation.").into_response()
            }
        };

        let json = match serde_json::to_value(&room) {
            Ok(json) => json,
            Err(_) => return StatusCode::BAD_REQUEST.into_response()
        };
        let note = Notification {
            notification_event: NotificationEvent::NewRoom,
            body: json,
            created_at: Utc::now(),
            display_value: None
        };
        BroadcastChannel::get().send_event_to_all(users, note).await;
        Json(room).into_response()
    }
}


pub async fn get_room_list_item_by_id(
    Extension(token): Extension<KeycloakToken<String>>,
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<Uuid>
) -> impl IntoResponse {
    let id = parse_uuid(&token.subject).unwrap();
    match state.room_repository.find_specific_joined_room(&id, &room_id).await {
        Ok(Some(room)) => Json(room).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(err) => HttpError::bad_request(err.to_string()).into_response()
    }
}

fn parse_uuid(subject: &str) -> Result<Uuid, HttpError> {
    Uuid::try_parse(subject).map_err(|_| HttpError::bad_request("Invalid token subject".to_string()))
}

async fn check_user_in_room(
    state: &Arc<AppState>,
    user_id: &Uuid,
    room_id: &Uuid,
) -> Result<(), HttpError> {
    let is_in = state
        .room_repository
        .is_user_in_room(user_id, room_id)
        .await
        .map_err(|_| HttpError::bad_request("Failed to check room access."))?;

    if is_in {
        Ok(())
    } else {
        Err(HttpError::unauthorized("Room not found or access denied."))
    }
}