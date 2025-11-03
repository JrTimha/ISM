use std::sync::Arc;
use axum::{Extension, Json};
use axum::extract::{Path, State, Multipart, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{Utc};
use log::{error, info};
use uuid::Uuid;
use bytes::Bytes;
use crate::api::errors::{ErrorCode, HttpError};
use crate::api::timeline::{msg_to_dto};
use crate::keycloak::decode::KeycloakToken;
use crate::model::{ChatRoomWithUserDTO, MembershipStatus, Message, MessageBody, NewRoom as UploadRoom, RoomType, RoomChangeBody, ChatRoomEntity, RoomMember, UploadResponse, SingleRoomSearchUserParams};
use crate::api::utils::{check_user_in_room, crop_image_from_center};
use crate::broadcast::{BroadcastChannel, Notification};
use crate::broadcast::NotificationEvent::{LeaveRoom, NewRoom, RoomChangeEvent};
use crate::core::AppState;


pub async fn get_users_in_room(
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<Uuid>
) -> impl IntoResponse {
    match state.room_repository.select_all_user_in_room(&room_id).await {
        Ok(users) => Json(users).into_response(),
        Err(err) => HttpError::bad_request(ErrorCode::RoomNotFound, err.to_string()).into_response()
    }
}

pub async fn get_joined_rooms(
    State(state): State<Arc<AppState>>,
    Extension(token): Extension<KeycloakToken<String>>,
) -> impl IntoResponse {
    match state.room_repository.get_joined_rooms(&token.subject).await {
        Ok(rooms) => Json(rooms).into_response(),
        Err(err) => HttpError::new(StatusCode::INTERNAL_SERVER_ERROR, ErrorCode::UnexpectedError, err.to_string()).into_response()
    }
}

pub async fn get_room_with_details(
    State(state): State<Arc<AppState>>,
    Extension(token): Extension<KeycloakToken<String>>,
    Path(room_id): Path<Uuid>
) -> impl IntoResponse {
    if let Err(err) = check_user_in_room(&state, &token.subject, &room_id).await {
        return err.into_response();
    }

    let res = tokio::try_join!( //executing 2 queries async
        state.room_repository.find_specific_joined_room(&room_id, &token.subject),
        state.room_repository.select_all_user_in_room(&room_id)
    );

    match res {
        Ok((room_option, users)) => {
            let chat_room = match room_option {
                Some(room) => room,
                None => return HttpError::new(StatusCode::NOT_FOUND, ErrorCode::RoomNotFound, "Room not found").into_response()
            };

            let room_details = ChatRoomWithUserDTO {
                id: chat_room.id,
                room_type: chat_room.room_type,
                room_name: chat_room.room_name.unwrap_or(String::from("Unnamed Chat")),
                room_image_url: chat_room.room_image_url,
                created_at: chat_room.created_at,
                latest_message: chat_room.latest_message,
                unread: chat_room.unread,
                latest_message_preview_text: chat_room.latest_message_preview_text,
                users: users,
            };
            Json(room_details).into_response()
        }
        Err(err) => {
            HttpError::bad_request(ErrorCode::RoomNotFound, err.to_string()).into_response()
        }
    }

}

pub async fn mark_room_as_read(
    State(state): State<Arc<AppState>>,
    Extension(token): Extension<KeycloakToken<String>>,
    Path(room_id): Path<Uuid>
) -> impl IntoResponse {
    let pl = state.room_repository.get_connection();
    match state.room_repository.update_user_read_status(pl, &room_id, &token.subject).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(_) => HttpError::bad_request(ErrorCode::UnexpectedError,"Can't update user read status.").into_response()
    }
}


pub async fn create_room(
    Extension(token): Extension<KeycloakToken<String>>,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<UploadRoom>
) -> impl IntoResponse {

    if !payload.invited_users.contains(&token.subject) {
        return HttpError::bad_request(ErrorCode::InvalidContent, "Sender ID is not in the list of invited users.").into_response();
    }

    match payload.room_type {
        RoomType::Single => {
            if payload.invited_users.len() != 2 {
                return HttpError::bad_request(ErrorCode::InvalidContent, "Personal rooms must have exactly two IDs (sender + one other).").into_response();
            }
        }
        RoomType::Group => {
            if payload.invited_users.len() < 2 {
                return HttpError::bad_request(ErrorCode::InvalidContent, "Groups must have more than one user.").into_response();
            }
        }
    }

    let room_entity = match state.room_repository.insert_room(payload.clone()).await {
        Ok(room) => room,
        Err(error) => {
            error!("{}", error);
            return HttpError::new(StatusCode::INTERNAL_SERVER_ERROR, ErrorCode::UnexpectedError, "Unable to persist the room.").into_response()
        }
    };

    let users = payload.invited_users;
    
    if room_entity.room_type == RoomType::Single {
        let other_user = match users.iter().find(|&&entry| entry != token.subject) {
            Some(other_user) => other_user,
            None => return HttpError::bad_request(ErrorCode::InvalidContent,"Can't find other user.").into_response(),
        };

        //sending 2 specific room views to the users, because private rooms are shown like another user
        let result = tokio::try_join!( //executing 2 queries async
        state.room_repository.find_specific_joined_room(&room_entity.id, &token.subject),
        state.room_repository.find_specific_joined_room(&room_entity.id, other_user)
        );
        match result {
            Ok((room_creator, room_participator)) => {
                if let (Some(creator_dto), Some(participator_dto)) = (room_creator, room_participator) {
                    let broadcast = BroadcastChannel::get();
                    
                    broadcast.send_event(Notification {
                        body: NewRoom {room: participator_dto},
                        created_at: Utc::now()
                    }, other_user).await;

                    broadcast.send_event(Notification {
                        body: NewRoom {room: creator_dto.clone()},
                        created_at: Utc::now()
                    }, &token.subject).await;

                    Json(creator_dto).into_response()
                } else {
                    HttpError::bad_request(ErrorCode::UnexpectedError,"Room for participator is null.").into_response()
                }
            }
            Err(error) => {
                error!("{}", error);
                HttpError::bad_request(ErrorCode::UnexpectedError,"Can't find the room.").into_response()
            }
        }

    } else { //is group room

        let room = match state.room_repository.find_specific_joined_room(&room_entity.id, &token.subject).await {
            Ok(Some(room)) => room,
            Ok(None) => return HttpError::bad_request(ErrorCode::UnexpectedError,"Room not found after creation.").into_response(),
            Err(error) => {
                error!("{}", error);
                return HttpError::bad_request(ErrorCode::UnexpectedError,"Room not found after creation.").into_response()
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
    match state.room_repository.find_specific_joined_room(&room_id, &token.subject).await {
        Ok(Some(room)) => Json(room).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(err) => HttpError::bad_request(ErrorCode::UnexpectedError, err.to_string()).into_response()
    }
}


pub async fn leave_room(
    Extension(token): Extension<KeycloakToken<String>>,
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<Uuid>
) -> impl IntoResponse {
    let result = tokio::try_join!( //executing 2 queries async
        state.room_repository.select_room(&room_id),
        state.room_repository.select_joined_user_in_room(&room_id)
    );
    let (room, users) = match result {
        Ok((room, users)) => (room, users),
        Err(error) => {
            error!("{}", error.to_string());
            return HttpError::bad_request(ErrorCode::InvalidContent,"Can't get room & user state.").into_response()
        }
    };
    let leaving_user = match users.iter().find(|user| user.id == token.subject) {
        Some(user) => {user.clone()}
        None => {
            return HttpError::new(StatusCode::UNAUTHORIZED, ErrorCode::InsufficientPermissions,"User not found in this room.").into_response();
        }
    };
    if room.room_type == RoomType::Single { //if someone leaves a single room, the whole room is getting wiped!
        handle_leave_private_room(state, room, users).await
    } else { //handle the group leave logic
        handle_leave_group_room(state, room, users, leaving_user).await
    }
}

async fn handle_leave_private_room(state: Arc<AppState>, room: ChatRoomEntity, users: Vec<RoomMember>) -> Response {
    if let Err(err) = state.message_repository.clear_chat_room_messages(&room.id).await {
        error!("Can't clear chat messages for this room: {}", err);
        return HttpError::bad_request(ErrorCode::UnexpectedError, "Unable to delete this room.").into_response();
    };
    let mut tx = state.room_repository.start_transaction().await.unwrap();
    if let Err(err) = state.room_repository.delete_room(&mut *tx, &room.id).await {
        error!("Can't delete room: {}", err);
        return HttpError::bad_request(ErrorCode::UnexpectedError, "Unable to change room membership state in db.").into_response();
    };
    let send_to: Vec<Uuid> = users.iter().map(|user| user.id).collect();
    BroadcastChannel::get().send_event_to_all(
        send_to,
        Notification {
            body: LeaveRoom {room_id: room.id},
            created_at: Utc::now()
        }
    ).await;
    tx.commit().await.unwrap();
    StatusCode::OK.into_response()
}

async fn handle_leave_group_room(state: Arc<AppState>, room: ChatRoomEntity, users: Vec<RoomMember>, mut leaving_user: RoomMember) -> Response {
    let mut tx = state.room_repository.start_transaction().await.unwrap();
    if let Err(err) = state.room_repository.remove_user_from_room(&mut *tx, &room.id, &leaving_user).await {
        error!("{}", err.to_string());
        return HttpError::bad_request(ErrorCode::UnexpectedError, "Unable to change room membership state in db.").into_response();
    }
    leaving_user.membership_status = MembershipStatus::Left;

    if users.len() == 1 { //last user, delete this room now
        if let Err(err) = state.message_repository.clear_chat_room_messages(&room.id).await {
            error!("Can't clear chat messages for this room: {}", err);
        };
        if let Err(err) = state.room_repository.delete_room(&mut *tx, &room.id).await {
            error!("Can't delete room: {}", err);
            return HttpError::bad_request(ErrorCode::UnexpectedError, "Unable to change room membership state in db.").into_response();
        };
        BroadcastChannel::get().send_event(
            Notification {
                body: LeaveRoom {room_id: room.id},
                created_at: Utc::now()
            },
            &leaving_user.id
        ).await;
        tx.commit().await.unwrap();

        //delete room image if it exists:
        if room.room_image_url.is_some() {
            let url = room.room_image_url.unwrap();
            match state.s3_bucket.delete_object(&url).await {
                Ok(_) => {
                    info!("Deleted image for room: {}", &room.id);
                },
                Err(err) => {
                    error!("Can't delete image of room: {}", err);
                }
            };
        }

        StatusCode::OK.into_response()
    } else { //find and handle the leaving user
        let message = match Message::new(room.id, leaving_user.id, MessageBody::RoomChange(RoomChangeBody::UserLeft {related_user: leaving_user.clone()})) {
            Ok(json) => json,
            Err(err) => {
                error!("{}", err.to_string());
                return HttpError::bad_request(ErrorCode::UnexpectedError, "Can't serialize message").into_response()
            }
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
        tx.commit().await.unwrap();
        StatusCode::OK.into_response()
    }
}


pub async fn invite_to_room(
    Extension(token): Extension<KeycloakToken<String>>,
    State(state): State<Arc<AppState>>,
    Path((room_id, user_id)): Path<(Uuid, Uuid)>
) -> impl IntoResponse {
    
    let result = tokio::try_join!( //executing 2 queries async
        state.room_repository.select_room(&room_id),
        state.room_repository.select_joined_user_in_room(&room_id)
    );
    let (room, users) = match result {
        Ok((room, users)) => (room, users),
        Err(error) => {
            error!("{}", error.to_string());
            return HttpError::bad_request(ErrorCode::UnexpectedError, "Can't get room & user state.").into_response()
        }
    };
    if room.room_type == RoomType::Single { 
        return HttpError::bad_request(ErrorCode::InvalidContent, "Room type single doesn't allow invites!").into_response();
    }
    //we have to check if the inviter is in the room and the invited user isn't!
    let user_to_find = users.iter().find(|user| user.id == token.subject);
    let user_to_exclude = users.iter().find(|user| user.id == user_id);
    match (user_to_find, user_to_exclude) {
        (Some(_inviter), None) => {} //we have checked the invite rules and continue
        _ => {
            return HttpError::bad_request(ErrorCode::InvalidContent,"User conditions not met in this room.").into_response();
        }
    };

    //add him to the room
    let user = match state.room_repository.add_user_to_room(&user_id, &room_id).await {
        Ok(user) => user,
        Err(err) => {
            error!("{}", err.to_string());
            return HttpError::bad_request(ErrorCode::UnexpectedError,"Unable to change room membership state in db.").into_response();
        }
    };

    //build room change message
    let message = match Message::new(room_id, user.id, MessageBody::RoomChange(RoomChangeBody::UserJoined {related_user: user.clone()})) {
        Ok(json) => json,
        Err(err) => {
            error!("{}", err.to_string());
            return HttpError::bad_request(ErrorCode::UnexpectedError,"Can't serialize message").into_response()
        }
    };
    //sending room change event to all previous users in the room
    let send_to: Vec<Uuid> = users.iter().map(|user| user.id).collect();
    save_message_and_broadcast(message, &state, send_to).await;


    //sending new room event to invited user
    let room_for_user = match state.room_repository.find_specific_joined_room(&room_id, &user_id).await {
        Ok(Some(room)) => room,
        Ok(None) => return HttpError::bad_request(ErrorCode::UnexpectedError,"Room not found after creation.").into_response(),
        Err(err) => {
            error!("{}", err.to_string());
            return HttpError::bad_request(ErrorCode::UnexpectedError,"Room not found after creation.").into_response()
        }
    };

    //notify the invited user:
    BroadcastChannel::get().send_event(
        Notification {
            body: NewRoom{room: room_for_user},
            created_at: Utc::now()
        },
        &user.id
    ).await;
    StatusCode::OK.into_response()
}

async fn save_message_and_broadcast(message: Message, state: &Arc<AppState>, to_users: Vec<Uuid>) -> Response {
    if let Err(err) = state.message_repository.insert_data(message.clone()).await {
        error!("{}", err.to_string());
        return HttpError::bad_request(ErrorCode::UnexpectedError,"Unable to persist the message.").into_response();
    };

    let mapped_msg = match msg_to_dto(message) {
        Ok(msg) => msg,
        Err(err) => {
            return HttpError::bad_request(ErrorCode::UnexpectedError,format!("Can't serialize message: {}", err)).into_response()
        }
    };
    let note = Notification {
        body: RoomChangeEvent{message: mapped_msg},
        created_at: Utc::now()
    };
    BroadcastChannel::get().send_event_to_all(to_users, note).await;
    StatusCode::OK.into_response()
}


pub async fn search_existing_single_room(
    Extension(token): Extension<KeycloakToken<String>>,
    State(state): State<Arc<AppState>>,
    Query(params): Query<SingleRoomSearchUserParams>,
) -> impl IntoResponse {
    match state.room_repository.find_room_between_users(&token.subject, &params.with_user).await {
        Ok(Some(room)) => (StatusCode::OK, room.to_string()).into_response(),
        Ok(None) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            error!("{}", e.to_string());
            HttpError::bad_request(ErrorCode::UnexpectedError,"Unexpected data query error.").into_response()
        }
    }
}

pub async fn save_room_image(
    Extension(token): Extension<KeycloakToken<String>>,
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<Uuid>,
    mut multipart: Multipart
) -> impl IntoResponse {
    if let Err(err) = check_user_in_room(&state, &token.subject, &room_id).await {
        return err.into_response();
    }

    let mut image_data: Option<Bytes> = None;

    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                if field.name() ==  Some("image") {
                    let data = match field.bytes().await {
                        Ok(data) => data,
                        Err(_) => {
                            return HttpError::bad_request(ErrorCode::UnexpectedError,"Error reading the image byte stream.").into_response()
                        }
                    };
                    image_data = Some(data);
                    break;
                }
            },
            Ok(None) => {
                break; //stream finished
            }
            Err(err) => { //read error
                error!("Bad image upload: {}", err.to_string());
                return HttpError::bad_request(ErrorCode::InvalidContent,"Can't extract image file.").into_response()
            }
        }
    }

    if let Some(image_data) = image_data {
        let img = match crop_image_from_center(&image_data, 500, 500) {
            Ok(img) => img,
            Err(err) => {
                return err.into_response()
            }
        };
        let object_id = format!("rooms/{}", room_id);
        if let Err(err) = state.s3_bucket.insert_object(&object_id, img).await {
            error!("{}", err.to_string());
            return HttpError::bad_request(ErrorCode::UnexpectedError,"Can't save image.").into_response()
        };
        if let Err(err) = state.room_repository.update_room_img_url(&room_id, &object_id).await{
            error!("{}", err.to_string());
            return HttpError::bad_request(ErrorCode::UnexpectedError,"Can't save image.").into_response()
        };
        let response = UploadResponse {
            image_url: object_id.clone(),
            image_name: format!("{}.png", object_id),
        };

        (StatusCode::CREATED, Json(response)).into_response()
    } else {
        HttpError::bad_request(ErrorCode::InvalidContent,"Required field 'image' not found in the upload.").into_response()
    }
}