use crate::broadcast::NotificationEvent::{LeaveRoom, RoomChangeEvent, UserReadChat};
use crate::broadcast::{BroadcastChannel, Notification};
use crate::core::AppState;
use crate::core::cursor::{CursorResults, next_cursor};
use crate::core::errors::AppError;
use crate::messaging::model::{MessageBody, MessageDto, MessageEntity, RoomChangeBody};
use crate::rooms::model::UploadResponse;
use crate::rooms::room::{
    ChatRoomDto, ChatRoomEntity, ChatRoomWithUserDTO, LastMessagePreviewText, NewRoom,
    RoomChangeType, RoomPaginationCursor, RoomType,
};
use crate::rooms::room_member::RoomMember;
use crate::utils::crop_image_from_center;
use bytes::Bytes;
use log::error;
use std::sync::Arc;
use uuid::Uuid;

pub struct RoomService;

impl RoomService {
    pub async fn get_users_in_room(
        state: Arc<AppState>,
        room_id: Uuid,
    ) -> Result<Vec<RoomMember>, AppError> {
        let users = state
            .room_repository
            .select_all_room_member(&room_id)
            .await
            .map_err(|_| AppError::NotFound("Room not found:".to_string()))?;
        Ok(users)
    }

    pub async fn get_joined_rooms(
        state: Arc<AppState>,
        client_id: Uuid,
        name_filter: Option<String>,
        cursor: RoomPaginationCursor,
        page_size: usize,
    ) -> Result<CursorResults<ChatRoomDto>, AppError> {
        let mut rooms = state
            .room_repository
            .get_joined_rooms(
                &client_id,
                name_filter.as_deref(),
                cursor,
                (page_size + 1) as i64,
            )
            .await?;

        let next_cursor = next_cursor(&mut rooms, page_size, |room| RoomPaginationCursor {
            last_seen_latest_message: room.latest_message,
            last_seen_room_id: Some(room.id),
        })
        .map_err(|e| AppError::Processing(format!("Cursor encoding failed: {}", e)))?;

        Ok(CursorResults {
            cursor: next_cursor,
            content: rooms.iter().map(|room| room.to_dto()).collect(),
        })
    }

    pub async fn get_room_with_details(
        state: Arc<AppState>,
        client_id: Uuid,
        room_id: Uuid,
    ) -> Result<ChatRoomWithUserDTO, AppError> {
        let (chat_room, users) = tokio::try_join!(
            //executing 2 queries async
            state
                .room_repository
                .find_specific_joined_room(&room_id, &client_id),
            state.room_repository.select_all_room_member(&room_id)
        )?;

        match chat_room {
            Some(room) => {
                let room_details = ChatRoomWithUserDTO {
                    room: room.to_dto(),
                    users,
                };
                Ok(room_details)
            }
            None => Err(AppError::NotFound("Room not found:".to_string())),
        }
    }

    pub async fn mark_room_as_read(
        state: Arc<AppState>,
        client_id: Uuid,
        room_id: Uuid,
    ) -> Result<(), AppError> {
        let pl = state.room_repository.get_connection();
        state
            .room_repository
            .update_user_read_status(pl, &room_id, &client_id)
            .await?;

        let room = state.room_repository.select_room(&room_id).await?;
        if room.latest_message.is_none() {
            return Ok(());
        }

        let users_in_room = state
            .room_repository
            .select_room_participants_ids(&room_id)
            .await?;
        BroadcastChannel::get()
            .send_event_to_all(
                users_in_room,
                Notification::new(UserReadChat {
                    user_id: client_id,
                    room_id,
                }),
            )
            .await;
        Ok(())
    }

    pub async fn get_read_states(
        state: Arc<AppState>,
        room_id: Uuid,
    ) -> Result<Vec<RoomMember>, AppError> {
        let users = state
            .room_repository
            .select_all_room_member(&room_id)
            .await?;
        let room = state.room_repository.select_room(&room_id).await?;
        let read_users: Vec<RoomMember> = users
            .into_iter()
            .filter(|user| user_has_read(user, room.latest_message))
            .collect();
        Ok(read_users)
    }

    pub async fn create_room(
        state: Arc<AppState>,
        client_id: Uuid,
        new_room: NewRoom,
    ) -> Result<ChatRoomDto, AppError> {
        let room_entity = state.room_repository.insert_room(new_room.clone()).await?;
        let creator_entity = state
            .user_repository
            .find_user_by_id(&client_id)
            .await?
            .ok_or_else(|| AppError::NotFound("UserID not found.".to_string()))?;
        let users = new_room.invited_users;

        if room_entity.room_type == RoomType::Single {
            let other_user = match users.iter().find(|&&entry| entry != client_id) {
                Some(other_user) => other_user,
                None => return Err(AppError::Validation("Can't find other user.".to_string())),
            };

            //sending 2 specific room views to the users, because private rooms are shown like another user
            let (room_client, room_receiver) = tokio::try_join!(
                //executing 2 queries async
                state
                    .room_repository
                    .find_specific_joined_room(&room_entity.id, &client_id),
                state
                    .room_repository
                    .find_specific_joined_room(&room_entity.id, other_user)
            )?;

            if let (Some(creator_room), Some(participator_room)) = (room_client, room_receiver) {
                let broadcast = BroadcastChannel::get();

                broadcast
                    .send_event(
                        Notification::new(crate::broadcast::NotificationEvent::NewRoom {
                            room: participator_room.to_dto(),
                            created_by: creator_entity.clone(),
                        }),
                        other_user,
                    )
                    .await;

                broadcast
                    .send_event(
                        Notification::new(crate::broadcast::NotificationEvent::NewRoom {
                            room: creator_room.to_dto(),
                            created_by: creator_entity,
                        }),
                        &client_id,
                    )
                    .await;

                Ok(creator_room.to_dto())
            } else {
                Err(AppError::Processing(
                    "Newly created room is null.".to_string(),
                ))
            }
        } else {
            //is group room
            let room_dto = room_entity.to_dto();
            BroadcastChannel::get()
                .send_event_to_all(
                    users,
                    Notification::new(crate::broadcast::NotificationEvent::NewRoom {
                        room: room_dto.clone(),
                        created_by: creator_entity.clone(),
                    }),
                )
                .await;
            Ok(room_dto)
        }
    }

    pub async fn get_room_list_item_by_id(
        state: Arc<AppState>,
        client_id: Uuid,
        room_id: Uuid,
    ) -> Result<ChatRoomDto, AppError> {
        let room = state
            .room_repository
            .find_specific_joined_room(&room_id, &client_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Room not found.".to_string()))?;
        Ok(room.to_dto())
    }

    pub async fn leave_room(
        state: Arc<AppState>,
        client_id: Uuid,
        room_id: Uuid,
    ) -> Result<(), AppError> {
        let (room, users) = tokio::try_join!(
            //executing 2 queries async
            state.room_repository.select_room(&room_id),
            state.room_repository.select_all_room_member(&room_id)
        )?;
        let leaving_user = match users.iter().find(|user| user.id == client_id) {
            Some(user) => user.clone(),
            None => {
                return Err(AppError::Forbidden(
                    "Client is not in this room.".to_string(),
                ));
            }
        };

        if room.room_type == RoomType::Single {
            //if someone leaves a single room, the whole room is getting wiped!
            handle_leave_private_room(state, room, users).await?;
            Ok(())
        } else {
            //handle the group leave logic
            handle_leave_group_room(state, room, users, leaving_user).await?;
            Ok(())
        }
    }

    pub async fn invite_to_room(
        state: Arc<AppState>,
        client_id: Uuid,
        room_id: Uuid,
        user_id: Uuid,
    ) -> Result<(), AppError> {
        let (room, users, creator) = tokio::try_join!(
            //executing 3 queries async
            state.room_repository.select_room(&room_id),
            state.room_repository.select_all_room_member(&room_id),
            state.user_repository.find_user_by_id(&client_id)
        )?;

        let creator_entity =
            creator.ok_or_else(|| AppError::NotFound("UserID not found.".to_string()))?;

        if room.room_type == RoomType::Single {
            return Err(AppError::Validation(
                "Private rooms doesn't allow invites!.".to_string(),
            ));
        };

        //we have to check if the inviter is in the room and the invited user isn't!
        users
            .iter()
            .find(|user| user.id == client_id)
            .ok_or_else(|| AppError::Forbidden("Client is not in this room.".to_string()))?;

        let user_to_exclude = users.iter().find(|user| user.id == user_id);
        if user_to_exclude.is_some() {
            return Err(AppError::Validation(
                "User is already in this room.".to_string(),
            ));
        }

        //1. add him to the room
        let mut tx = state.room_repository.start_transaction().await?;
        let user = state
            .room_repository
            .add_user_to_room(&mut *tx, &user_id, &room_id)
            .await?;
        let preview_text = LastMessagePreviewText::RoomChange {
            sender_username: user.display_name.clone(),
            room_change_type: RoomChangeType::JOIN,
        };
        state
            .room_repository
            .update_last_room_message(&mut *tx, &room_id, &preview_text)
            .await?;

        //2. build room change message and send it to all previous users in the room
        let message = MessageEntity::new(
            room_id,
            user.id,
            MessageBody::RoomChange(RoomChangeBody::UserJoined {
                related_user: user.clone(),
            }),
        );
        state
            .chat_repository
            .insert_message(&mut *tx, &message)
            .await?;

        let send_to: Vec<Uuid> = users.iter().map(|user| user.id).collect();
        tx.commit().await?;

        save_room_change_message_and_broadcast(message, send_to, preview_text).await?;
        state.cache.invalidate_room_context(&room_id).await?;

        //sending new room event to invited user
        let room_for_user = state
            .room_repository
            .find_specific_joined_room(&room_id, &user_id)
            .await?
            .ok_or_else(|| {
                AppError::Processing("Unable to find room for the invited user.".to_string())
            })?;

        BroadcastChannel::get()
            .send_event(
                Notification::new(crate::broadcast::NotificationEvent::NewRoom {
                    room: room_for_user.to_dto(),
                    created_by: creator_entity,
                }),
                &user.id,
            )
            .await;

        Ok(())
    }

    pub async fn find_existing_single_room(
        state: Arc<AppState>,
        client_id: &Uuid,
        with_user: &Uuid,
    ) -> Result<Option<Uuid>, AppError> {
        let room_id = state
            .room_repository
            .find_room_between_users(client_id, with_user)
            .await?;
        Ok(room_id)
    }

    pub async fn set_room_image(
        state: Arc<AppState>,
        room_id: Uuid,
        image_data: Bytes,
    ) -> Result<UploadResponse, AppError> {
        let img = crop_image_from_center(&image_data, 500, 500).map_err(|err| {
            error!("Unable to crop image: {}", err.to_string());
            AppError::Processing("Unable to crop image.".to_string())
        })?;

        let object_id = format!("{}/{}", state.env.object_db_config.bucket_name, room_id);
        if let Err(err) = state
            .s3_bucket
            .insert_object(&room_id.to_string(), img)
            .await
        {
            error!("{}", err.to_string());
            return Err(AppError::S3("Unable save image in s3 bucket.".to_string()));
        };
        state
            .room_repository
            .update_room_img_url(&room_id, &object_id)
            .await?;
        let response = UploadResponse {
            image_url: object_id.clone(),
            image_name: format!("{}.jpeg", object_id),
        };
        Ok(response)
    }
}

// Helper used by `get_read_states` — extracted for easier unit testing of the read logic.
fn user_has_read(user: &RoomMember, room_latest: Option<chrono::DateTime<chrono::Utc>>) -> bool {
    match (room_latest, user.last_message_read_at) {
        (Some(latest_msg_time), Some(read_time)) => read_time >= latest_msg_time,
        (Some(_), None) => false,
        (None, _) => true,
    }
}

async fn handle_leave_private_room(
    state: Arc<AppState>,
    room: ChatRoomEntity,
    users: Vec<RoomMember>,
) -> Result<(), AppError> {
    let mut tx = state.room_repository.start_transaction().await?;
    state
        .chat_repository
        .delete_room_messages(&mut *tx, &room.id)
        .await?;
    state
        .room_repository
        .delete_room(&mut *tx, &room.id)
        .await?;
    tx.commit().await?;

    state.cache.invalidate_room_context(&room.id).await?;

    let send_to: Vec<Uuid> = users.iter().map(|user| user.id).collect();
    BroadcastChannel::get()
        .send_event_to_all(send_to, Notification::new(LeaveRoom { room_id: room.id }))
        .await;
    Ok(())
}

async fn handle_leave_group_room(
    state: Arc<AppState>,
    room: ChatRoomEntity,
    users: Vec<RoomMember>,
    leaving_user: RoomMember,
) -> Result<(), AppError> {
    let mut tx = state.room_repository.start_transaction().await?;

    let preview_message = LastMessagePreviewText::RoomChange {
        sender_username: leaving_user.display_name.clone(),
        room_change_type: RoomChangeType::LEAVE,
    };
    state
        .room_repository
        .remove_user_from_room(&mut *tx, &room.id, &leaving_user.id, &preview_message)
        .await?;

    if users.len() == 1 {
        //last user, delete this room now
        state
            .chat_repository
            .delete_room_messages(&mut *tx, &room.id)
            .await?;
        state
            .room_repository
            .delete_room(&mut *tx, &room.id)
            .await?;
        tx.commit().await?;

        state.cache.invalidate_room_context(&room.id).await?;

        BroadcastChannel::get()
            .send_event(
                Notification::new(LeaveRoom { room_id: room.id }),
                &leaving_user.id,
            )
            .await;

        //delete room image if it exists:
        if let Some(_url) = room.room_image_url {
            state
                .s3_bucket
                .delete_object(&room.id.to_string())
                .await
                .map_err(|_| {
                    AppError::Processing("Unable to delete image from room".to_string())
                })?;
        }

        Ok(())
    } else {
        //find and handle the leaving user

        let message = MessageEntity::new(
            room.id,
            leaving_user.id,
            MessageBody::RoomChange(RoomChangeBody::UserLeft {
                related_user: leaving_user.clone(),
            }),
        );
        state
            .chat_repository
            .insert_message(&mut *tx, &message)
            .await?;
        tx.commit().await?;

        let send_to: Vec<Uuid> = users
            .iter()
            .filter(|user| user.id != leaving_user.id)
            .map(|user| user.id)
            .collect();
        save_room_change_message_and_broadcast(message, send_to, preview_message).await?;

        state.cache.invalidate_room_context(&room.id).await?;

        //send ack to the leaving user
        BroadcastChannel::get()
            .send_event(
                Notification::new(LeaveRoom { room_id: room.id }),
                &leaving_user.id,
            )
            .await;

        Ok(())
    }
}

async fn save_room_change_message_and_broadcast(
    message: MessageEntity,
    to_users: Vec<Uuid>,
    preview_text: LastMessagePreviewText,
) -> Result<(), AppError> {
    let mapped_msg = MessageDto::from(message);
    let notification = Notification::new(RoomChangeEvent {
        message: mapped_msg,
        room_preview_text: preview_text,
    });
    BroadcastChannel::get()
        .send_event_to_all(to_users, notification)
        .await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rooms::room_member::RoomMember;
    use chrono::{Duration, Utc};
    use uuid::Uuid;

    fn make_member(read_at: Option<chrono::DateTime<Utc>>) -> RoomMember {
        RoomMember {
            id: Uuid::new_v4(),
            display_name: "test".to_string(),
            profile_picture: None,
            joined_at: Some(Utc::now()),
            last_message_read_at: read_at,
        }
    }

    #[test]
    fn user_has_read_when_no_latest_message() {
        let user = make_member(None);
        let result = user_has_read(&user, None);
        assert!(
            result,
            "When room has no latest message, every user should be considered read"
        );
    }

    #[test]
    fn user_has_read_when_read_time_ge_latest() {
        let latest = Utc::now();
        let read_time = latest + Duration::seconds(1);
        let user = make_member(Some(read_time));
        assert!(user_has_read(&user, Some(latest)));
    }

    #[test]
    fn user_has_not_read_when_read_time_before_latest() {
        let latest = Utc::now();
        let read_time = latest - Duration::seconds(10);
        let user = make_member(Some(read_time));
        assert!(!user_has_read(&user, Some(latest)));
    }

    #[test]
    fn user_has_not_read_when_no_read_time_and_latest_present() {
        let latest = Utc::now();
        let user = make_member(None);
        assert!(!user_has_read(&user, Some(latest)));
    }
}
