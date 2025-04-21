use std::sync::Arc;
use axum::{Extension, Json};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{Utc};
use log::{error};
use uuid::Uuid;
use crate::api::errors::{HttpError};
use crate::api::timeline::{msg_to_dto};
use crate::database::{RoomRepository};
use crate::keycloak::decode::KeycloakToken;
use crate::model::{ChatRoomWithUserDTO, MembershipStatus, Message, MsgType, NewRoom as UploadRoom, RoomType, SystemBody};
use crate::api::utils::{check_user_in_room, parse_uuid};
use crate::broadcast::{BroadcastChannel, Notification};
use crate::broadcast::NotificationEvent::{LeaveRoom, NewRoom, RoomChangeEvent};
use crate::core::AppState;


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
            let room_details = ChatRoomWithUserDTO {
                id: room.id,
                room_type: room.room_type,
                room_name: room.room_name,
                room_image_url: room.room_image_url,
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
    Json(payload): Json<UploadRoom>
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
    users.retain(|&user| user != id); //don't send it to the creator, he will get it from the http response

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
                    
                    BroadcastChannel::get().send_event(Notification {
                        body: NewRoom{room: participator_dto},
                        created_at: Utc::now()
                    }, other_user).await;
                    
                    BroadcastChannel::get().send_event(Notification {
                        body: NewRoom {room: creator_dto},
                        created_at: Utc::now()
                    }, &id).await;
                    
                    StatusCode::CREATED.into_response()
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

        BroadcastChannel::get().send_event_to_all(
            users,
            Notification {
                body: NewRoom{room: room.clone()},
                created_at: Utc::now()
            }
        ).await;
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


pub async fn leave_room(
    Extension(token): Extension<KeycloakToken<String>>,
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<Uuid>
) -> impl IntoResponse {
    let id = parse_uuid(&token.subject).unwrap();
    let res = tokio::try_join!( //executing 2 queries async
        state.room_repository.select_room(&room_id),
        state.room_repository.select_joined_user_in_room(&room_id)
    );

    match res {
        Ok((room, users)) => {
            let mut leaving_user = match users.iter().find(|user| user.id == id) {
              Some(user) => {user.clone()}
              None => {
                  return HttpError::bad_request("User not found in this room.").into_response();
              }
            };
            if let Err(err) = state.room_repository.remove_user_from_room(&room_id, &leaving_user).await {
                error!("{}", err.to_string());
                return HttpError::bad_request("Unable to change room membership state in db.").into_response();
            }
            leaving_user.membership_status = MembershipStatus::Left;

            let body_json = match serde_json::to_string(&SystemBody::UserLeft { related_user: leaving_user.clone() }) {
                Ok(json) => json,
                Err(err) => {
                    error!("{}", err.to_string());
                    return HttpError::bad_request("Can't serialize message").into_response()
                }
            };

            let message = Message {
                chat_room_id: room.id,
                message_id: Uuid::new_v4(),
                sender_id: leaving_user.id,
                msg_body: body_json,
                msg_type: MsgType::System.to_string(),
                created_at: Utc::now(),
            };
            let send_to: Vec<Uuid> = users.iter().map(|user| user.id).collect();
            save_message_and_broadcast(message, &state, send_to).await;
            
            BroadcastChannel::get().send_event(
                Notification {
                    body: LeaveRoom {room_id: room.id},
                    created_at: Utc::now()
                }, 
                &leaving_user.id
            ).await;
            
            StatusCode::OK.into_response()
        }
        Err(error) => {
            error!("{}", error.to_string());
            HttpError::bad_request("Can't get room & user state.").into_response()
        }
    }

}


pub async fn invite_to_room(
    Extension(token): Extension<KeycloakToken<String>>,
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<Uuid>,
    Path(user_id): Path<Uuid>
) -> impl IntoResponse {
    let id = parse_uuid(&token.subject).unwrap();

    match state.room_repository.select_joined_user_in_room(&room_id).await {
        Ok(users) => {
            let user_to_find = users.iter().find(|user| user.id == id);
            let user_to_exclude = users.iter().find(|user| user.id == user_id);

            match (user_to_find, user_to_exclude) {
                (Some(_inviter), None) => {} //we have checked the invite rules and continue
                _ => {
                    return HttpError::bad_request("User conditions not met in this room.").into_response();
                }
            }

            let user = match state.room_repository.add_user_to_room(&room_id, &user_id).await {
                Ok(user) => user,
                Err(err) => {
                    error!("{}", err.to_string());
                    return HttpError::bad_request("Unable to change room membership state in db.").into_response();
                }
            };

            let body_json = match serde_json::to_string(&SystemBody::UserJoined { related_user: user.clone() }) {
                Ok(json) => json,
                Err(err) => {
                    error!("{}", err.to_string());
                    return HttpError::bad_request("Can't serialize message").into_response()
                }
            };
            
            //sending room change event to all previous users in the room
            let send_to: Vec<Uuid> = users.iter().map(|user| user.id).collect();
            let message = Message {
                chat_room_id: room_id,
                message_id: Uuid::new_v4(),
                sender_id: user.id,
                msg_body: body_json,
                msg_type: MsgType::System.to_string(),
                created_at: Utc::now(),
            };
            save_message_and_broadcast(message, &state, send_to).await;
            
            //sending new room event to invited user
            let room_for_user = match state.room_repository.find_specific_joined_room(&room_id, &user_id).await {
                Ok(Some(room)) => room,
                Ok(None) => return HttpError::bad_request("Room not found after creation.").into_response(),
                Err(err) => {
                    error!("{}", err.to_string());
                    return HttpError::bad_request("Room not found after creation.").into_response()
                }
            };

            let note = Notification {
                body: NewRoom{room: room_for_user},
                created_at: Utc::now()
            };
            BroadcastChannel::get().send_event(note, &user.id).await;
            StatusCode::OK.into_response()
        }
        Err(error) => {
            error!("{}", error.to_string());
            HttpError::bad_request("Can't get room & user state.").into_response()
        }
    }
}

async fn save_message_and_broadcast(message: Message, state: &Arc<AppState>, to_users: Vec<Uuid>) -> Response {
    if let Err(err) = state.message_repository.insert_data(message.clone()).await {
        error!("{}", err.to_string());
        return HttpError::bad_request("Unable to persist the message.").into_response();
    };

    let mapped_msg = match msg_to_dto(message) {
        Ok(msg) => msg,
        Err(err) => {
            return HttpError::bad_request(format!("Can't serialize message: {}", err)).into_response()
        }
    };
    let note = Notification {
        body: RoomChangeEvent{message: mapped_msg},
        created_at: Utc::now()
    };
    BroadcastChannel::get().send_event_to_all(to_users, note).await;
    StatusCode::OK.into_response()
}