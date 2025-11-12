use std::sync::Arc;
use bytes::Bytes;
use chrono::Utc;
use log::{error};
use uuid::Uuid;
use crate::broadcast::{BroadcastChannel, Notification};
use crate::broadcast::NotificationEvent::{LeaveRoom, RoomChangeEvent};
use crate::core::AppState;
use crate::errors::{AppError};
use crate::messaging::model::{Message, MessageBody, RoomChangeBody};
use crate::model::{ChatRoomDto, ChatRoomEntity, ChatRoomWithUserDTO, LastMessagePreviewText, MembershipStatus, NewRoom, RoomChangeType, RoomMember, RoomType, UploadResponse};
use crate::utils::crop_image_from_center;

pub struct RoomService;

impl RoomService {

    pub async fn get_users_in_room(state: Arc<AppState>, room_id: Uuid, ) -> Result<Vec<RoomMember>, AppError> {
        let users = state.room_repository.select_all_user_in_room(&room_id).await.map_err(|_| AppError::NotFound("Room not found:".to_string()))?;
        Ok(users)
    }

    pub async fn get_joined_rooms(state: Arc<AppState>, client_id: Uuid, ) -> Result<Vec<ChatRoomDto>, AppError> {
        let rooms =  state.room_repository.get_joined_rooms(&client_id).await?;
        Ok(rooms.iter().map(|room| room.to_dto()).collect())
    }

    pub async fn get_room_with_details(state: Arc<AppState>, client_id: Uuid, room_id: Uuid) -> Result<ChatRoomWithUserDTO, AppError> {

        let (chat_room, users) = tokio::try_join!( //executing 2 queries async
            state.room_repository.find_specific_joined_room(&room_id, &client_id),
            state.room_repository.select_all_user_in_room(&room_id)
        )?;

        match chat_room {
            Some(room) => {
                let room_details = ChatRoomWithUserDTO { room: room.to_dto(), users };
                Ok(room_details)
            },
            None => Err(AppError::NotFound("Room not found:".to_string()))
        }
    }

    pub async fn mark_room_as_read(state: Arc<AppState>, client_id: Uuid, room_id: Uuid) -> Result<(), AppError> {
        let pl = state.room_repository.get_connection();
        state.room_repository.update_user_read_status(pl, &room_id, &client_id).await?;
        Ok(())
    }

    pub async fn create_room(state: Arc<AppState>, client_id: Uuid, new_room: NewRoom) -> Result<ChatRoomDto, AppError> {
        let room_entity = state.room_repository.insert_room(new_room.clone()).await?;
        let users = new_room.invited_users;

        if room_entity.room_type == RoomType::Single {
            let other_user = match users.iter().find(|&&entry| entry != client_id) {
                Some(other_user) => other_user,
                None => return Err(AppError::ValidationError("Can't find other user.".to_string()))
            };

            //sending 2 specific room views to the users, because private rooms are shown like another user
            let (room_client, room_receiver) = tokio::try_join!( //executing 2 queries async
                state.room_repository.find_specific_joined_room(&room_entity.id, &client_id),
                state.room_repository.find_specific_joined_room(&room_entity.id, other_user)
            )?;

            if let (Some(creator_room), Some(participator_room)) = (room_client, room_receiver) {

                let broadcast = BroadcastChannel::get();

                broadcast.send_event(Notification {
                    body: crate::broadcast::NotificationEvent::NewRoom {room: participator_room.to_dto()},
                    created_at: Utc::now()
                }, other_user).await;

                broadcast.send_event(Notification {
                    body: crate::broadcast::NotificationEvent::NewRoom {room: creator_room.to_dto()},
                    created_at: Utc::now()
                }, &client_id).await;

                Ok(creator_room.to_dto())
            } else {
                Err(AppError::ProcessingError("Newly created room is null.".to_string()))
            }
        } else { //is group room
            let room_dto = room_entity.to_dto();
            BroadcastChannel::get().send_event_to_all(
                users,
                Notification {
                    body: crate::broadcast::NotificationEvent::NewRoom {room: room_dto.clone()},
                    created_at: Utc::now()
                }
            ).await;
            Ok(room_dto)
        }
    }

    pub async fn get_room_list_item_by_id(state: Arc<AppState>, client_id: Uuid, room_id: Uuid) -> Result<ChatRoomDto, AppError> {
        let room = state.room_repository.find_specific_joined_room(&room_id, &client_id).await?.ok_or_else(|| {
            AppError::NotFound("Room not found.".to_string())
        })?;
        Ok(room.to_dto())
    }

    pub async fn leave_room(state: Arc<AppState>, client_id: Uuid, room_id: Uuid) -> Result<(), AppError> {
        let (room, users) = tokio::try_join!( //executing 2 queries async
            state.room_repository.select_room(&room_id),
            state.room_repository.select_joined_user_in_room(&room_id)
        )?;
        let leaving_user = match users.iter().find(|user| user.id == client_id) {
            Some(user) => user.clone(),
            None => {
                return Err(AppError::Blocked("Client is not in this room.".to_string()))
            }
        };

        if room.room_type == RoomType::Single { //if someone leaves a single room, the whole room is getting wiped!
            handle_leave_private_room(state, room, users).await?;
            Ok(())
        } else { //handle the group leave logic
            handle_leave_group_room(state, room, users, leaving_user).await?;
            Ok(())
        }
    }

    pub async fn invite_to_room(state: Arc<AppState>, client_id: Uuid, room_id: Uuid, user_id: Uuid) -> Result<(), AppError> {
        let (room, users) = tokio::try_join!( //executing 2 queries async
            state.room_repository.select_room(&room_id),
            state.room_repository.select_joined_user_in_room(&room_id)
        )?;

        if room.room_type == RoomType::Single {
            return Err(AppError::ValidationError("Private rooms doesn't allow invites!.".to_string()))
        };

        //we have to check if the inviter is in the room and the invited user isn't!
        users.iter().find(|user| user.id == client_id).ok_or_else(|| {
            AppError::Blocked("Client is not in this room.".to_string())
        })?;


        let user_to_exclude = users.iter().find(|user| user.id == user_id);
        if user_to_exclude.is_some() {
            return Err(AppError::BadRequest("User is already in this room.".to_string()))
        }

        //1. add him to the room
        let mut tx = state.room_repository.start_transaction().await?;
        let user = state.room_repository.add_user_to_room(&mut *tx, &user_id, &room_id).await?;
        let preview_text = LastMessagePreviewText::RoomChange { sender_username: user.display_name.clone(), room_change_type: RoomChangeType::JOIN};
        let preview_str = serde_json::to_string(&preview_text).map_err(|_| {
            AppError::ProcessingError("Can't serialize room preview text".to_string())
        })?;
        state.room_repository.update_last_room_message(&mut *tx, &room_id, &preview_str).await?;
        tx.commit().await?;

        //2. build room change message and send it to all previous users in the room
        let message = Message::new(room_id, user.id, MessageBody::RoomChange(RoomChangeBody::UserJoined {related_user: user.clone()}))
            .map_err(|_| AppError::ProcessingError("Unable to create room message".to_string()))?;

        let send_to: Vec<Uuid> = users.iter().map(|user| user.id).collect();
        save_room_change_message_and_broadcast(message, &state, send_to, preview_text).await?;
        state.cache.add_user_to_room_cache(&user.id, &room_id).await?;

        //sending new room event to invited user
        let room_for_user = state.room_repository.find_specific_joined_room(&room_id, &user_id).await?.ok_or_else(|| {
            AppError::ProcessingError("Unable to find room for the invited user.".to_string())
        })?;

        BroadcastChannel::get().send_event(
            Notification {
                body: crate::broadcast::NotificationEvent::NewRoom {room: room_for_user.to_dto()},
                created_at: Utc::now()
            },
            &user.id
        ).await;

        Ok(())
    }

    pub async fn find_existing_single_room(state: Arc<AppState>, client_id: &Uuid, with_user: &Uuid) -> Result<Option<Uuid>, AppError> {
        let room_id = state.room_repository.find_room_between_users(client_id, with_user).await?;
        Ok(room_id)
    }

    pub async fn set_room_image(state: Arc<AppState>, room_id: Uuid, image_data: Bytes) -> Result<UploadResponse, AppError> {

        let img = crop_image_from_center(&image_data, 500, 500).map_err(|err| {
            error!("Unable to crop image: {}", err.to_string());
            AppError::ProcessingError("Unable to crop image.".to_string())
        })?;

        let object_id = format!("{}/{}", state.env.object_db_config.bucket_name, room_id);
        if let Err(err) = state.s3_bucket.insert_object(&object_id, img).await {
            error!("{}", err.to_string());
            return Err(AppError::S3Error("Unable save image in s3 bucket.".to_string()))
        };
        state.room_repository.update_room_img_url(&room_id, &object_id).await?;
        let response = UploadResponse {
            image_url: object_id.clone(),
            image_name: format!("{}.jpeg", object_id),
        };
        Ok(response)
    }

}

async fn handle_leave_private_room(state: Arc<AppState>, room: ChatRoomEntity, users: Vec<RoomMember>) -> Result<(), AppError> {
    let mut tx = state.room_repository.start_transaction().await?;
    state.room_repository.delete_room(&mut *tx, &room.id).await?;
    tx.commit().await?;
    state.message_repository.clear_chat_room_messages(&room.id).await?;

    state.cache.set_user_for_room(&room.id, &vec![]).await?;

    let send_to: Vec<Uuid> = users.iter().map(|user| user.id).collect();
    BroadcastChannel::get().send_event_to_all(
        send_to,
        Notification {
            body: LeaveRoom {room_id: room.id},
            created_at: Utc::now()
        }
    ).await;
    Ok(())
}

async fn handle_leave_group_room(state: Arc<AppState>, room: ChatRoomEntity, users: Vec<RoomMember>, mut leaving_user: RoomMember) -> Result<(), AppError> {
    let mut tx = state.room_repository.start_transaction().await?;

    let preview_message = LastMessagePreviewText::RoomChange { sender_username: leaving_user.display_name.clone(), room_change_type: RoomChangeType::LEAVE };
    let preview_text = serde_json::to_string(&preview_message).map_err(|err| {
        AppError::ProcessingError(format!("Unable to serialize last message preview text: {}", err.to_string()))
    })?;

    state.room_repository.remove_user_from_room(&mut *tx, &room.id, &leaving_user.id, &preview_text).await?;
    leaving_user.membership_status = MembershipStatus::Left;

    if users.len() == 1 { //last user, delete this room now
        state.message_repository.clear_chat_room_messages(&room.id).await?;
        state.room_repository.delete_room(&mut *tx, &room.id).await?;
        tx.commit().await?;

        state.cache.set_user_for_room(&room.id, &vec![]).await?;

        BroadcastChannel::get().send_event(
            Notification {
                body: LeaveRoom {room_id: room.id},
                created_at: Utc::now()
            },
            &leaving_user.id
        ).await;

        //delete room image if it exists:
        if let Some(_url) = room.room_image_url {
            state.s3_bucket.delete_object(&room.id.to_string()).await
                .map_err(|_| AppError::ProcessingError("Unable to delete image from room".to_string()))?;
        }

        Ok(())
    } else { //find and handle the leaving user

        let message = Message::new(room.id, leaving_user.id, MessageBody::RoomChange(RoomChangeBody::UserLeft {related_user: leaving_user.clone()}))
            .map_err(|_err| AppError::ProcessingError("Unable to create room message".to_string()))?;

        let send_to: Vec<Uuid> = users.iter().filter(|user| user.id != leaving_user.id).map(|user| user.id).collect();
        save_room_change_message_and_broadcast(message, &state, send_to, preview_message).await?;
        tx.commit().await?;

        state.cache.remove_user_from_room_cache(&leaving_user.id, &room.id).await?;

        //send ack to the leaving user
        BroadcastChannel::get().send_event(
            Notification {
                body: LeaveRoom {room_id: room.id},
                created_at: Utc::now()
            },
            &leaving_user.id
        ).await;

        Ok(())
    }
}

async fn save_room_change_message_and_broadcast(message: Message, state: &Arc<AppState>, to_users: Vec<Uuid>, preview_text: LastMessagePreviewText) -> Result<(), AppError> {
    state.message_repository.insert_data(message.clone()).await?;

    let mapped_msg = message.to_dto().map_err(|_| {
        AppError::ProcessingError("Unable to cast message to dto.".to_string())
    })?;

    let notification = Notification {
        body: RoomChangeEvent{message: mapped_msg, room_preview_text: preview_text},
        created_at: Utc::now()
    };

    BroadcastChannel::get().send_event_to_all(to_users, notification).await;
    Ok(())
}