use std::sync::Arc;
use chrono::Utc;
use uuid::Uuid;
use crate::broadcast::{BroadcastChannel, Notification};
use crate::broadcast::NotificationEvent::{FriendRequestAccepted, FriendRequestReceived};
use crate::core::AppState;
use crate::core::cursor::{encode_cursor, CursorResults};
use crate::errors::{AppError};
use crate::user_relationship::model::{Relationship, RelationshipState, User, UserPaginationCursor, UserRelationshipEntity, UserWithRelationshipDto};


pub struct UserService;

impl UserService {

    /// Asynchronously queries a list of users based on a given username query, including their relationship type with the current user.
    ///
    /// This function fetches users whose names match the given `username_query` and paginates the results based on the supplied `cursor`.
    /// The results returned are wrapped in a `CursorResults` structure, facilitating pagination with cursors.
    ///
    /// # Pagination Behavior
    /// - A fixed page size of 20 is used for each query. An additional record is fetched to determine if there are more results beyond the current page.
    /// - If more than `page_size` results are retrieved, the last record (used to identify the continuation cursor) is removed before returning the page content.
    ///
    pub async fn query_user_by_name(
        state: Arc<AppState>,
        current_user_id: &Uuid,
        username_query: &str,
        cursor: UserPaginationCursor
    ) -> Result<CursorResults<UserWithRelationshipDto>, AppError> {

        let page_size: usize = 20;
        let query_page_size = page_size + 1;

        let mut users = state.user_repository
            .find_user_by_name_with_relationship_type(current_user_id, username_query, query_page_size as i64, cursor)
            .await?;

        let next_cursor_string = if users.len() > page_size {
            users.pop();
            users.last().map(|last_user| {
                let next_page_cursor_struct = UserPaginationCursor {
                    last_seen_id: Some(last_user.r_user.id.clone()),
                    last_seen_name: Some(last_user.r_user.display_name.clone()),
                };
                encode_cursor(&next_page_cursor_struct).map_err(|e| AppError::ProcessingError(format!("Cursor encoding failed: {}", e)))
            }).transpose()?
        } else {
            None
        };

        let mapped_users = users.iter().map(|item| {
            item.to_dto(current_user_id)
        }).collect();

        Ok(CursorResults {
            next_cursor: next_cursor_string,
            content: mapped_users,
        })
    }

    pub async fn query_user_by_id(
        state: Arc<AppState>,
        current_user_id: &Uuid,
        user_id: &Uuid,
    ) -> Result<UserWithRelationshipDto, AppError> {

        let db_user = state
            .user_repository
            .find_user_by_id_with_relationship_type(current_user_id, user_id)
            .await?;

        let user = db_user.ok_or_else(|| {
            AppError::NotFound(format!("User with ID {} not found.", user_id))
        })?;

        Ok(user.to_dto(current_user_id))
    }

    pub async fn get_open_friend_requests(
        state: Arc<AppState>,
        current_user_id: &Uuid,
    ) -> Result<Vec<User>, AppError> {
        let users = state.user_repository.select_open_friend_requests(current_user_id).await?;
        Ok(users)
    }

    pub async fn get_friends(
        state: Arc<AppState>,
        current_user_id: &Uuid,
    ) -> Result<Vec<User>, AppError> {
        let users = state.user_repository.find_users_with_specific_relationship(current_user_id, RelationshipState::FRIEND).await?;
        Ok(users)
    }

    pub async fn add_friend(
        state: Arc<AppState>,
        sender_id: Uuid,
        receiver_id: Uuid,
    ) -> Result<(), AppError> {
        let mut tx = state.user_repository.start_transaction().await?;
        let relationship = state.user_repository.search_for_relationship(&mut tx, &sender_id, &receiver_id).await?;
        if relationship.is_some() { //don't handle this request further when the users are in a relationship
            return match relationship.unwrap().state {
                RelationshipState::A_BLOCKED => Err(AppError::ValidationError("Relationship between users is blocked.".to_string())),
                RelationshipState::B_BLOCKED => Err(AppError::ValidationError("Relationship between users is blocked.".to_string())),
                RelationshipState::ALL_BLOCKED => Err(AppError::ValidationError("Relationship between users is blocked.".to_string())),
                RelationshipState::FRIEND => Ok(()),
                RelationshipState::A_INVITED => Ok(()),
                RelationshipState::B_INVITED => Ok(()),
            }
        }
        let (user_a_id, user_b_id) = if sender_id < receiver_id {
            (sender_id, receiver_id)
        } else {
            (receiver_id, sender_id)
        };

        let relationship_state = if sender_id == user_a_id {
            RelationshipState::A_INVITED
        } else {
            RelationshipState::B_INVITED
        };

        let init_relationship = UserRelationshipEntity {
            user_a_id,
            user_b_id,
            state: relationship_state,
            relationship_change_timestamp: Utc::now(),
        };

        state.user_repository.insert_relationship(&mut tx, &init_relationship).await?;

        tx.commit().await?;
        let client_dto = state.user_repository.find_user_by_id(&sender_id).await?.ok_or_else(|| {
            AppError::NotFound(format!("User with ID {} not found.", sender_id))
        })?;
        BroadcastChannel::get().send_event(
            Notification {
                body: FriendRequestReceived {from_user: client_dto},
                created_at: Utc::now()
            },
            &receiver_id
        ).await;
        Ok(())
    }

    pub async fn accept_friend_request(
        state: Arc<AppState>,
        client_id: Uuid,
        sender_id: Uuid,
    ) -> Result<(), AppError> {
        let mut tx = state.user_repository.start_transaction().await?;
        let relationship = state.user_repository.search_for_relationship(&mut tx, &client_id, &sender_id).await?.ok_or_else(|| {
            AppError::NotFound("Relationship between these users not found.".to_string())
        })?;

        let is_accepter_user_a = client_id == relationship.user_a_id;
        match (relationship.state, is_accepter_user_a) {
            (RelationshipState::B_INVITED, true) => {}, //valid state
            (RelationshipState::A_INVITED, false) => {}, //valid state
            _ => { //everything else is invalid
                return Err(AppError::ValidationError(
                    "Cannot accept this request. Invalid state or user.".to_string(),
                ));
            }
        }
        state.user_repository.update_relationship_state(
            &mut tx,
            &relationship.user_a_id,
            &relationship.user_b_id,
            RelationshipState::FRIEND
        ).await?;

        state.user_repository.increment_friends_count(&mut tx, &relationship.user_a_id).await?;
        state.user_repository.increment_friends_count(&mut tx, &relationship.user_b_id).await?;
        tx.commit().await?;

        let client_dto = state.user_repository.find_user_by_id(&client_id).await?.ok_or_else(|| {
            AppError::NotFound(format!("User with ID {} not found.", client_id))
        })?;

        BroadcastChannel::get().send_event(
            Notification {
                body: FriendRequestAccepted {from_user: client_dto},
                created_at: Utc::now()
            },
            &sender_id
        ).await;

        Ok(())
    }

    pub async fn reject_friend_request(
        state: Arc<AppState>,
        client_id: Uuid,
        sender_id: Uuid,
    ) -> Result<(), AppError> {
        let mut tx = state.user_repository.start_transaction().await?;
        let relationship = state.user_repository.search_for_relationship(&mut tx, &client_id, &sender_id).await?.ok_or_else(|| {
            AppError::NotFound("Relationship between these users not found.".to_string())
        })?;

        let is_rejecter_user_a = client_id == relationship.user_a_id;
        match (relationship.state.clone(), is_rejecter_user_a) {
            (RelationshipState::B_INVITED, true) => {}, //valid state
            (RelationshipState::A_INVITED, false) => {}, //valid state
            _ => { //everything else is invalid
                return Err(AppError::ValidationError(
                    "Cannot reject this request. Invalid state or user.".to_string(),
                ));
            }
        }
        state.user_repository.delete_relationship_state(&mut tx, relationship).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn remove_friend(
        state: Arc<AppState>,
        client_id: Uuid,
        sender_id: Uuid,
    ) -> Result<(), AppError> {
        let mut tx = state.user_repository.start_transaction().await?;
        let relationship = state.user_repository.search_for_relationship(&mut tx, &client_id, &sender_id).await?.ok_or_else(|| {
            AppError::NotFound("Relationship between these users not found.".to_string())
        })?;

        if relationship.state == RelationshipState::FRIEND {
            state.user_repository.decrement_friends_count(&mut tx, &relationship.user_a_id).await?;
            state.user_repository.decrement_friends_count(&mut tx, &relationship.user_b_id).await?;
            state.user_repository.delete_relationship_state(&mut tx, relationship).await?;
            tx.commit().await?;
        } else {
            return Err(AppError::ValidationError("These users aren't in a friend relationship.".to_string()));
        }
        Ok(())
    }

    pub async fn ignore_user(
        state: Arc<AppState>,
        client_id: Uuid,
        ignored_user_id: Uuid,
    ) -> Result<Relationship, AppError> {
        let mut tx = state.user_repository.start_transaction().await?;
        let relationship = state.user_repository.search_for_relationship(&mut tx, &client_id, &ignored_user_id).await?;

        if let Some(rel) = relationship {

            let is_client_user_a = client_id == rel.user_a_id;

            let new_state = match (rel.state, is_client_user_a) {
                (RelationshipState::ALL_BLOCKED, _) => return Ok(Relationship::ClientBlocked), //Both blocked
                (RelationshipState::A_BLOCKED, true) => return Ok(Relationship::ClientBlocked), //client is A and blocked B
                (RelationshipState::B_BLOCKED, false) => return Ok(Relationship::ClientBlocked), //client is B and blocked A
                (RelationshipState::A_BLOCKED, false) => RelationshipState::ALL_BLOCKED,
                (RelationshipState::B_BLOCKED, true) => RelationshipState::ALL_BLOCKED,
                (RelationshipState::FRIEND, _) => {
                    state.user_repository.decrement_friends_count(&mut tx, &rel.user_a_id).await?;
                    state.user_repository.decrement_friends_count(&mut tx, &rel.user_b_id).await?;

                    if is_client_user_a {
                        RelationshipState::A_BLOCKED
                    } else {
                        RelationshipState::B_BLOCKED
                    }
                },
                (RelationshipState::A_INVITED, _) | (RelationshipState::B_INVITED, _) => {
                    if is_client_user_a {
                        RelationshipState::A_BLOCKED
                    } else {
                        RelationshipState::B_BLOCKED
                    }
                }
            };
            let entity = state.user_repository.update_relationship_state(
                &mut tx,
                &rel.user_a_id,
                &rel.user_b_id,
                new_state
            ).await?;
            tx.commit().await?;
            Ok(entity.resolve_relationship_state(&client_id))
        } else { //no relationship found, create one
            let (user_a_id, user_b_id) = if client_id < ignored_user_id {
                (client_id, ignored_user_id)
            } else {
                (ignored_user_id, client_id)
            };

            let relationship_state = if client_id == user_a_id {
                RelationshipState::A_BLOCKED
            } else {
                RelationshipState::B_BLOCKED
            };

            let init_relationship = UserRelationshipEntity {
                user_a_id,
                user_b_id,
                state: relationship_state.clone(),
                relationship_change_timestamp: Utc::now(),
            };
            state.user_repository.insert_relationship(&mut tx, &init_relationship).await?;
            tx.commit().await?;
            Ok(init_relationship.resolve_relationship_state(&client_id))
        }
    }

    pub async fn undo_ignore(
        state: Arc<AppState>,
        client_id: Uuid,
        ignored_user_id: Uuid,
    ) -> Result<Option<Relationship>, AppError> {
        let mut tx = state.user_repository.start_transaction().await?;
        let relationship = state
            .user_repository
            .search_for_relationship(&mut tx, &client_id, &ignored_user_id)
            .await?
            .ok_or_else(|| {
                AppError::NotFound("No block relationship found to undo.".to_string())
            })?;
        let is_client_user_a = client_id == relationship.user_a_id;
        let state = match (relationship.state.clone(), is_client_user_a) {
            (RelationshipState::ALL_BLOCKED, true) => { // Client was A, only B blocking now
                let entity = state.user_repository.update_relationship_state(
                    &mut tx,
                    &relationship.user_a_id,
                    &relationship.user_b_id,
                    RelationshipState::B_BLOCKED,
                ).await?;
                Some(entity)
            },
            (RelationshipState::ALL_BLOCKED, false) => { // Client was B, only A blocking now
                let entity = state.user_repository.update_relationship_state(
                    &mut tx,
                    &relationship.user_a_id,
                    &relationship.user_b_id,
                    RelationshipState::A_BLOCKED,
                ).await?;
                Some(entity)
            },

            (RelationshipState::A_BLOCKED, true) | (RelationshipState::B_BLOCKED, false) => { // Fall 2: only client blocked, remove relationship
                state.user_repository.delete_relationship_state(
                    &mut tx,
                    relationship
                ).await?;
                None
            },
            (RelationshipState::A_BLOCKED, false) | (RelationshipState::B_BLOCKED, true) => { //client was blocked by another user
                return Err(AppError::Blocked(
                    "You cannot undo a block placed on you by another user.".to_string(),
                ));
            },
            _ => { // some other state, no undo possible
                return Err(AppError::ValidationError(
                    "No active block from your side found to undo.".to_string(),
                ));
            }
        };
        tx.commit().await?;
        match state {
            Some(entity) => { Ok(Some(entity.resolve_relationship_state(&client_id))) },
            None => Ok(None)
        }
    }

    pub async fn get_blocked_users(
        state: Arc<AppState>,
        current_user_id: &Uuid,
        users_to_validate: &Vec<Uuid>
    ) -> Result<Vec<Uuid>, AppError> {
        let users = state.user_repository.find_blocked_relationships(current_user_id, users_to_validate).await?;
        Ok(users)
    }

}