use crate::core::AppState;
use crate::core::cursor::{CursorResults, encode_cursor};
use crate::core::errors::AppError;
use crate::rooms::room::RoomType;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

/// Row of the *active* share-target section: a room the client can already send to —
/// either a group room or a friend's existing 1-1 room. Ordered by `active_at DESC`.
/// `user_id` is the friend behind a 1-1 room (`None` for groups); the share target is
/// always the existing `room_id`. Populated by `RoomRepository::active_share_targets`.
#[derive(Debug, sqlx::FromRow)]
pub struct ActiveShareRow {
    /// Display name of the other user (1-1) or the room name (group); groups may be unnamed.
    pub name: Option<String>,
    pub room_id: Uuid,
    pub image_url: Option<String>,
    pub active_at: DateTime<Utc>,
    pub is_group: bool,
    pub user_id: Option<Uuid>,
}

/// Row of the *inactive* share-target section: a friend the client has no 1-1 room with
/// yet. Ordered by `display_name ASC`. Sharing requires creating the room first.
/// Populated by `RoomRepository::inactive_share_targets`.
#[derive(Debug, sqlx::FromRow)]
pub struct InactiveShareRow {
    pub name: String,
    pub user_id: Uuid,
    pub image_url: Option<String>,
}

/// A single suggestion of where the client can send shared content (like an Instagram
/// "share to chat" sheet). Merges friends and group rooms into one list; `target` tells
/// the client whether to post into an existing room or to create one first.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareTarget {
    /// Other user's display name (1-1) or the room name (group); a group may be unnamed.
    pub name: Option<String>,
    pub image_url: Option<String>,
    pub target: ShareTargetRef,
}

/// What the client must do to deliver content to a [`ShareTarget`].
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ShareTargetRef {
    /// An existing room — share via `POST /api/send-msg` with this `roomId`.
    Room { room_id: Uuid, room_type: RoomType },
    /// A friend without a 1-1 room yet — create it via `POST /api/rooms/create-room`
    /// (`NewRoom`) for this `userId`, then send into the returned room.
    User { user_id: Uuid },
}

impl ShareTarget {
    fn from_active(row: ActiveShareRow) -> Self {
        let room_type = if row.is_group {
            RoomType::Group
        } else {
            RoomType::Single
        };
        ShareTarget {
            name: row.name,
            image_url: row.image_url,
            target: ShareTargetRef::Room {
                room_id: row.room_id,
                room_type,
            },
        }
    }

    fn from_inactive(row: InactiveShareRow) -> Self {
        ShareTarget {
            name: Some(row.name),
            image_url: row.image_url,
            target: ShareTargetRef::User {
                user_id: row.user_id,
            },
        }
    }
}

/// Which section of the merged share list the next page resumes in. The list is
/// two-phase: active rooms first, then inactive friends.
#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SharePhase {
    /// Rooms with activity (groups + friends with an existing 1-1 room), `active_at DESC`.
    #[default]
    Active,
    /// Friends without a 1-1 room, `displayName ASC`.
    Inactive,
}

/// Keyset cursor for the two-phase share-target list. The active section paginates over
/// `(active_at, room_id) DESC`; once it is exhausted the inactive section paginates over
/// `(name, user_id) ASC`. `phase` records which section the next page resumes in; the
/// default (`Active`, no bounds) starts at the top of the list.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareTargetCursor {
    pub phase: SharePhase,
    pub last_active_at: Option<DateTime<Utc>>,
    pub last_name: Option<String>,
    pub last_id: Option<Uuid>,
}

pub struct ShareService;

impl ShareService {
    /// Builds one page of share targets by merging two sources into a single
    /// cursor-paginated list:
    /// 1. **Active** — group rooms + friends with an existing 1-1 room, ordered by
    ///    recent activity. These resolve to an existing `room_id`.
    /// 2. **Inactive** — friends without a 1-1 room, ordered alphabetically. These
    ///    require a `NewRoom` POST before a message can be sent.
    ///
    /// The two sections have different sort axes, so each is a focused keyset query
    /// (`active_share_targets` / `inactive_share_targets`) and the cursor's `phase`
    /// records which one to resume. A boundary page may run both queries to fill up to
    /// `page_size`; all other pages run exactly one.
    pub async fn get_share_targets(
        state: Arc<AppState>,
        client_id: Uuid,
        name_filter: Option<String>,
        cursor: ShareTargetCursor,
        page_size: usize,
    ) -> Result<CursorResults<ShareTarget>, AppError> {
        let name = name_filter.as_deref();
        let mut content: Vec<ShareTarget> = Vec::with_capacity(page_size);

        // ── Phase 1: active section (rooms with recent activity) ──────────────
        if cursor.phase == SharePhase::Active {
            let mut rows = state
                .room_repository
                .active_share_targets(
                    &client_id,
                    name,
                    cursor.last_active_at,
                    cursor.last_id,
                    (page_size + 1) as i64,
                )
                .await?;

            if rows.len() > page_size {
                // More active rows remain — stay in the active phase.
                rows.truncate(page_size);
                let next = rows.last().map(|last| ShareTargetCursor {
                    phase: SharePhase::Active,
                    last_active_at: Some(last.active_at),
                    last_name: None,
                    last_id: Some(last.room_id),
                });
                content.extend(rows.into_iter().map(ShareTarget::from_active));
                return Self::encode(content, next);
            }

            // Active section fits entirely on this page.
            content.extend(rows.into_iter().map(ShareTarget::from_active));

            if content.len() >= page_size {
                // Page already full; the inactive section starts on the next page.
                let next = ShareTargetCursor {
                    phase: SharePhase::Inactive,
                    ..Default::default()
                };
                return Self::encode(content, Some(next));
            }
            // Otherwise fall through and fill the remainder from the inactive section.
        }

        // ── Phase 2: inactive section (friends without a 1-1 room) ────────────
        let remaining = page_size - content.len();
        // Resuming mid-inactive keeps the cursor bounds; arriving from the active phase
        // starts the inactive section from the beginning.
        let (cursor_name, cursor_id) = if cursor.phase == SharePhase::Inactive {
            (cursor.last_name.clone(), cursor.last_id)
        } else {
            (None, None)
        };

        let mut rows = state
            .room_repository
            .inactive_share_targets(
                &client_id,
                name,
                cursor_name,
                cursor_id,
                (remaining + 1) as i64,
            )
            .await?;

        let next = if rows.len() > remaining {
            rows.truncate(remaining);
            rows.last().map(|last| ShareTargetCursor {
                phase: SharePhase::Inactive,
                last_active_at: None,
                last_name: Some(last.name.clone()),
                last_id: Some(last.user_id),
            })
        } else {
            None
        };

        content.extend(rows.into_iter().map(ShareTarget::from_inactive));
        Self::encode(content, next)
    }

    fn encode(
        content: Vec<ShareTarget>,
        next: Option<ShareTargetCursor>,
    ) -> Result<CursorResults<ShareTarget>, AppError> {
        let cursor = match next {
            Some(c) => Some(
                encode_cursor(&c)
                    .map_err(|e| AppError::Processing(format!("Cursor encoding failed: {e}")))?,
            ),
            None => None,
        };
        Ok(CursorResults { cursor, content })
    }
}
