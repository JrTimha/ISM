use crate::broadcast::NotificationEvent;
use crate::broadcast::NotificationEvent::{LeaveRoom, RoomChangeEvent, UserReadChat};
use crate::core::cursor::{CursorResults, next_cursor};
use crate::core::errors::AppError;
use crate::core::{Database, Service};
use crate::messaging::ChatRepository;
use crate::messaging::model::{FirstMessageBody, MessageBody, MessageDto, MessageEntity, RoomChangeBody};
use crate::object_storage::ObjectStorage;
use crate::rooms::model::UploadResponse;
use crate::rooms::room::{ChatRoomDto, ChatRoomEntity, ChatRoomWithUserDTO, LastMessagePreviewText, NewRoom, RoomChangeType, RoomPaginationCursor, RoomType};
use crate::rooms::room_member::RoomMember;
use crate::rooms::{RoomNotifier, RoomRepository};
use crate::users::UserRepository;
use crate::utils::crop_image_from_center;
use crate::{notify_room, notify_user};
use bytes::Bytes;
use std::collections::HashSet;
use tracing::error;
use uuid::Uuid;

/// Rooms: creation, membership, read state and room images.
///
/// Holds [`UserRepository`] even though users are another domain's concern. That is deliberate:
/// what it needs from `users` is a single query (which of these ids has blocked me?), not a use
/// case, and depending on the repository instead of `UserService` keeps the service graph acyclic
/// — `UserService` already depends on this service. See [`crate::core::Service`].
#[derive(Clone)]
pub struct RoomService {
    /// Present only because this service owns transactions that span several repositories.
    db: Database,
    rooms: RoomRepository,
    chats: ChatRepository,
    users: UserRepository,
    notifier: RoomNotifier,
    storage: ObjectStorage,
    /// The bucket name, not the whole `ObjectStorageConfig` — a service takes the slice of
    /// configuration it uses.
    bucket: String,
}

impl Service for RoomService {
    const NAME: &'static str = "RoomService";
}

impl RoomService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: Database,
        rooms: RoomRepository,
        chats: ChatRepository,
        users: UserRepository,
        notifier: RoomNotifier,
        storage: ObjectStorage,
        bucket: String,
    ) -> Self {
        Self {
            db,
            rooms,
            chats,
            users,
            notifier,
            storage,
            bucket,
        }
    }

    /// Rejects a caller that is not currently in the room.
    ///
    /// The single place room authorization is decided. It used to be `utils::check_user_in_room`,
    /// called from four handlers — which meant a fifth handler forgetting to call it was a silent
    /// authorization hole rather than a compile error.
    async fn ensure_member(&self, client_id: &Uuid, room_id: &Uuid) -> Result<(), AppError> {
        if self.rooms.is_user_in_room(client_id, room_id).await? {
            Ok(())
        } else {
            Err(AppError::Forbidden("Invalid permissions to interact with this room".to_string()))
        }
    }

    pub async fn get_users_in_room(&self, client_id: Uuid, room_id: Uuid) -> Result<Vec<RoomMember>, AppError> {
        self.ensure_member(&client_id, &room_id).await?;
        let users = self
            .rooms
            .select_all_room_member(&room_id)
            .await
            .map_err(|_| AppError::NotFound("Room not found:".to_string()))?;
        Ok(users)
    }

    pub async fn get_joined_rooms(
        &self,
        client_id: Uuid,
        name_filter: Option<String>,
        cursor: RoomPaginationCursor,
        page_size: usize,
    ) -> Result<CursorResults<ChatRoomDto>, AppError> {
        let mut rooms = self
            .rooms
            .get_joined_rooms(&client_id, name_filter.as_deref(), cursor, (page_size + 1) as i64)
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

    pub async fn get_room_with_details(&self, client_id: Uuid, room_id: Uuid) -> Result<ChatRoomWithUserDTO, AppError> {
        let (chat_room, users) = tokio::try_join!(
            //executing 2 queries async
            self.rooms.find_specific_joined_room(&room_id, &client_id),
            self.rooms.select_all_room_member(&room_id)
        )?;

        match chat_room {
            Some(room) => {
                let room_details = ChatRoomWithUserDTO { room: room.to_dto(), users };
                Ok(room_details)
            }
            None => Err(AppError::NotFound("Room not found:".to_string())),
        }
    }

    pub async fn mark_room_as_read(&self, client_id: Uuid, room_id: Uuid) -> Result<(), AppError> {
        self.rooms.update_user_read_status(self.db.pool(), &room_id, &client_id).await?;

        let room = self.rooms.select_room(&room_id).await?;
        if room.latest_message.is_none() {
            return Ok(());
        }

        notify_room!(self.notifier, &room_id, UserReadChat { user_id: client_id, room_id });
        Ok(())
    }

    pub async fn get_read_states(&self, client_id: Uuid, room_id: Uuid) -> Result<Vec<RoomMember>, AppError> {
        self.ensure_member(&client_id, &room_id).await?;
        let users = self.rooms.select_all_room_member(&room_id).await?;
        let room = self.rooms.select_room(&room_id).await?;
        let read_users: Vec<RoomMember> = users.into_iter().filter(|user| user_has_read(user, room.latest_message)).collect();
        Ok(read_users)
    }

    /// Creates a room and announces it to its members.
    ///
    /// Owns the whole rule set, including the parts that used to sit in the handler: the creator
    /// must be among the invitees, users who have blocked the creator are dropped from the invite
    /// list, and a room type's cardinality (`Single` is exactly two people and only one may exist
    /// per pair) is enforced. Those are decisions about what a room *is*, and they have to hold for
    /// every caller — not only for requests that arrive through this one HTTP handler.
    pub async fn create_room(&self, client_id: Uuid, mut new_room: NewRoom) -> Result<ChatRoomDto, AppError> {
        if !new_room.invited_users.contains(&client_id) {
            return Err(AppError::Validation("Sender ID is not in the list of invited users.".to_string()));
        }

        // Users who blocked the creator never learn about the room.
        let ignored = self.users.find_blocked_relationships(&client_id, &new_room.invited_users).await?;
        let filter_set: HashSet<_> = ignored.iter().collect();
        new_room.invited_users.retain(|uuid| !filter_set.contains(uuid));

        match new_room.room_type {
            RoomType::Single => {
                if new_room.invited_users.len() != 2 {
                    return Err(AppError::Validation(
                        "Personal rooms must have exactly two IDs (sender + one other).".to_string(),
                    ));
                }
                let other_user = new_room
                    .invited_users
                    .iter()
                    .find(|&&id| id != client_id)
                    .ok_or_else(|| AppError::Validation("Personal rooms must contain another user.".to_string()))?;
                if self.find_existing_single_room(&client_id, other_user).await?.is_some() {
                    return Err(AppError::Validation("User already has an active personal chat.".to_string()));
                }
            }
            RoomType::Group => {
                if new_room.invited_users.len() < 2 {
                    return Err(AppError::Validation("Groups must have more than one user.".to_string()));
                }
            }
        }

        let creator_entity = self
            .users
            .find_user_by_id(&client_id)
            .await?
            .ok_or_else(|| AppError::NotFound("UserID not found.".to_string()))?;

        // Atomic: room + participants (+ optional first message) are created together,
        // so a failing message insert never leaves a half-created room behind.
        let mut tx = self.db.begin().await?;
        let room_entity = self.rooms.insert_room(&mut tx, &new_room).await?;

        let first_message = match &new_room.first_message {
            Some(body) => {
                let msg_body = match body.clone() {
                    FirstMessageBody::Text(text) => MessageBody::Text(text),
                    FirstMessageBody::Media(media) => MessageBody::Media(media),
                };
                let entity = MessageEntity::new(room_entity.id, client_id, msg_body);
                let preview_text = first_message_preview_text(body, creator_entity.display_name.clone());
                self.chats.insert_message(&mut *tx, &entity).await?;
                self.rooms
                    .apply_message_to_room(&mut tx, &room_entity.id, &preview_text, &entity.sender_id, entity.created_at)
                    .await?;
                Some(MessageDto::from(entity))
            }
            None => None,
        };
        tx.commit().await?;

        let users = new_room.invited_users;

        if room_entity.room_type == RoomType::Single {
            let other_user = match users.iter().find(|&&entry| entry != client_id) {
                Some(other_user) => other_user,
                None => return Err(AppError::Validation("Can't find other user.".to_string())),
            };

            //sending 2 specific room views to the users, because private rooms are shown like another user
            let (room_client, room_receiver) = tokio::try_join!(
                //executing 2 queries async
                self.rooms.find_specific_joined_room(&room_entity.id, &client_id),
                self.rooms.find_specific_joined_room(&room_entity.id, other_user)
            )?;

            if let (Some(creator_room), Some(participator_room)) = (room_client, room_receiver) {
                notify_user!(
                    self.notifier,
                    other_user,
                    NotificationEvent::NewRoom {
                        room: participator_room.to_dto(),
                        created_by: creator_entity.clone(),
                        first_message: first_message.clone(),
                    }
                );
                notify_user!(
                    self.notifier,
                    &client_id,
                    NotificationEvent::NewRoom {
                        room: creator_room.to_dto(),
                        created_by: creator_entity,
                        first_message,
                    }
                );

                Ok(creator_room.to_dto())
            } else {
                Err(AppError::Processing("Newly created room is null.".to_string()))
            }
        } else {
            //is group room
            let room_dto = room_entity.to_dto();
            self.notifier
                .notify_users(
                    users,
                    NotificationEvent::NewRoom {
                        room: room_dto.clone(),
                        created_by: creator_entity.clone(),
                        first_message,
                    },
                )
                .await;
            Ok(room_dto)
        }
    }

    pub async fn get_room_list_item_by_id(&self, client_id: Uuid, room_id: Uuid) -> Result<ChatRoomDto, AppError> {
        let room = self
            .rooms
            .find_specific_joined_room(&room_id, &client_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Room not found.".to_string()))?;
        Ok(room.to_dto())
    }

    pub async fn leave_room(&self, client_id: Uuid, room_id: Uuid) -> Result<(), AppError> {
        let (room, users) = tokio::try_join!(
            //executing 2 queries async
            self.rooms.select_room(&room_id),
            self.rooms.select_all_room_member(&room_id)
        )?;
        let leaving_user = match users.iter().find(|user| user.id == client_id) {
            Some(user) => user.clone(),
            None => {
                return Err(AppError::Forbidden("Client is not in this room.".to_string()));
            }
        };

        if room.room_type == RoomType::Single {
            //if someone leaves a single room, the whole room is getting wiped!
            self.leave_private_room(room, users).await
        } else {
            //handle the group leave logic
            self.leave_group_room(room, users, leaving_user).await
        }
    }

    /// Adds a user to a group room and announces it.
    ///
    /// The block check moved here from the handler for the same reason as in [`Self::create_room`]:
    /// "a user who blocked you cannot be pulled into a room by you" is a property of inviting, not
    /// of one HTTP route.
    pub async fn invite_to_room(&self, client_id: Uuid, room_id: Uuid, user_id: Uuid) -> Result<(), AppError> {
        let blocked = self.users.find_blocked_relationships(&client_id, &vec![user_id]).await?;
        if blocked.contains(&user_id) {
            return Err(AppError::Forbidden("User is blocked.".to_string()));
        }

        let (room, users, creator) = tokio::try_join!(
            //executing 3 queries async
            self.rooms.select_room(&room_id),
            self.rooms.select_all_room_member(&room_id),
            self.users.find_user_by_id(&client_id)
        )?;

        let creator_entity = creator.ok_or_else(|| AppError::NotFound("UserID not found.".to_string()))?;

        if room.room_type == RoomType::Single {
            return Err(AppError::Validation("Private rooms doesn't allow invites!.".to_string()));
        };

        //we have to check if the inviter is in the room and the invited user isn't!
        users
            .iter()
            .find(|user| user.id == client_id)
            .ok_or_else(|| AppError::Forbidden("Client is not in this room.".to_string()))?;

        let user_to_exclude = users.iter().find(|user| user.id == user_id);
        if user_to_exclude.is_some() {
            return Err(AppError::Validation("User is already in this room.".to_string()));
        }

        //1. add him to the room
        let mut tx = self.db.begin().await?;
        let user = self.rooms.add_user_to_room(&mut tx, &user_id, &room_id).await?;
        let preview_text = LastMessagePreviewText::RoomChange {
            sender_username: user.display_name.clone(),
            room_change_type: RoomChangeType::JOIN,
        };
        self.rooms.update_last_room_message(&mut tx, &room_id, &preview_text).await?;

        //2. build room change message and send it to all previous users in the room
        let message = MessageEntity::new(
            room_id,
            user.id,
            MessageBody::RoomChange(RoomChangeBody::UserJoined { related_user: user.clone() }),
        );
        self.chats.insert_message(&mut *tx, &message).await?;

        let send_to: Vec<Uuid> = users.iter().map(|user| user.id).collect();
        tx.commit().await?;

        // Membership changed, so the cached participant snapshot is stale — drop it before
        // anything reacts to the events below.
        self.notifier.invalidate(&room_id).await?;
        self.notifier.notify_users(send_to, room_change_event(message, preview_text)).await;

        //sending new room event to invited user
        let room_for_user = self
            .rooms
            .find_specific_joined_room(&room_id, &user_id)
            .await?
            .ok_or_else(|| AppError::Processing("Unable to find room for the invited user.".to_string()))?;

        notify_user!(
            self.notifier,
            &user.id,
            NotificationEvent::NewRoom {
                room: room_for_user.to_dto(),
                created_by: creator_entity,
                first_message: None,
            }
        );

        Ok(())
    }

    pub async fn find_existing_single_room(&self, client_id: &Uuid, with_user: &Uuid) -> Result<Option<Uuid>, AppError> {
        let room_id = self.rooms.find_room_between_users(client_id, with_user).await?;
        Ok(room_id)
    }

    pub async fn set_room_image(&self, client_id: Uuid, room_id: Uuid, image_data: Bytes) -> Result<UploadResponse, AppError> {
        self.ensure_member(&client_id, &room_id).await?;

        let img = crop_image_from_center(&image_data, 500, 500).map_err(|err| {
            error!(error = %err, "Unable to crop image");
            AppError::Processing("Unable to crop image.".to_string())
        })?;

        let object_id = format!("{}/{}", self.bucket, room_id);
        if let Err(err) = self.storage.insert_object(&room_id.to_string(), img).await {
            error!(error = %err, "Image processing failed");
            return Err(AppError::S3("Unable save image in s3 bucket.".to_string()));
        };
        self.rooms.update_room_img_url(&room_id, &object_id).await?;
        let response = UploadResponse {
            image_url: object_id.clone(),
            image_name: format!("{}.jpeg", object_id),
        };
        Ok(response)
    }

    /// A 1-1 room has no meaning once one side leaves, so it is deleted outright.
    async fn leave_private_room(&self, room: ChatRoomEntity, users: Vec<RoomMember>) -> Result<(), AppError> {
        let mut tx = self.db.begin().await?;
        self.chats.delete_room_messages(&mut *tx, &room.id).await?;
        self.rooms.delete_room(&mut tx, &room.id).await?;
        tx.commit().await?;

        self.notifier.invalidate(&room.id).await?;

        let send_to: Vec<Uuid> = users.iter().map(|user| user.id).collect();
        self.notifier.notify_users(send_to, LeaveRoom { room_id: room.id }).await;
        Ok(())
    }

    async fn leave_group_room(&self, room: ChatRoomEntity, users: Vec<RoomMember>, leaving_user: RoomMember) -> Result<(), AppError> {
        let mut tx = self.db.begin().await?;

        let preview_message = LastMessagePreviewText::RoomChange {
            sender_username: leaving_user.display_name.clone(),
            room_change_type: RoomChangeType::LEAVE,
        };
        self.rooms.remove_user_from_room(&mut tx, &room.id, &leaving_user.id, &preview_message).await?;

        if users.len() == 1 {
            //last user, delete this room now
            self.chats.delete_room_messages(&mut *tx, &room.id).await?;
            self.rooms.delete_room(&mut tx, &room.id).await?;
            tx.commit().await?;

            self.notifier.invalidate(&room.id).await?;
            notify_user!(self.notifier, &leaving_user.id, LeaveRoom { room_id: room.id });

            //delete room image if it exists:
            if let Some(_url) = room.room_image_url {
                self.storage
                    .delete_object(&room.id.to_string())
                    .await
                    .map_err(|_| AppError::Processing("Unable to delete image from room".to_string()))?;
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
            self.chats.insert_message(&mut *tx, &message).await?;
            tx.commit().await?;

            let send_to: Vec<Uuid> = users.iter().filter(|user| user.id != leaving_user.id).map(|user| user.id).collect();

            self.notifier.invalidate(&room.id).await?;
            self.notifier.notify_users(send_to, room_change_event(message, preview_message)).await;

            //send ack to the leaving user
            notify_user!(self.notifier, &leaving_user.id, LeaveRoom { room_id: room.id });

            Ok(())
        }
    }
}

/// Builds the room preview text for an optional first message sent on room creation.
/// Mirrors `MessageService::generate_room_preview_text`, but for the restricted
/// `FirstMessageBody` (no `Reply` in a brand-new room).
fn first_message_preview_text(body: &FirstMessageBody, sender_username: String) -> LastMessagePreviewText {
    match body {
        FirstMessageBody::Text(text) => LastMessagePreviewText::Text {
            sender_username,
            text: text.text.clone(),
        },
        FirstMessageBody::Media(media) => LastMessagePreviewText::Media {
            sender_username,
            media_type: media.media_type.clone(),
        },
    }
}

/// Wraps a persisted room-change message in the event clients render it with.
fn room_change_event(message: MessageEntity, preview_text: LastMessagePreviewText) -> NotificationEvent {
    RoomChangeEvent {
        message: MessageDto::from(message),
        room_preview_text: preview_text,
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
        assert!(result, "When room has no latest message, every user should be considered read");
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
