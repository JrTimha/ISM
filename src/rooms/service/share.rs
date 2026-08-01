//! Builds the "share to chat" suggestion list.
//!
//! The row and response types this service maps between used to be declared in this file, which
//! meant `RoomRepository` imported its own `FromRow` targets back out of the service layer. They
//! now live in `rooms::entity` and `rooms::response` like every other domain's.

use crate::core::Service;
use crate::core::cursor::{CursorResults, encode_cursor};
use crate::core::errors::AppError;
use crate::rooms::RoomRepository;
use crate::rooms::model::{SharePhase, ShareTargetCursor};
use crate::rooms::response::ShareTargetResponse;
use uuid::Uuid;

/// Builds the "share to chat" suggestion list.
#[derive(Clone)]
pub struct ShareService {
    rooms: RoomRepository,
}

impl Service for ShareService {
    const NAME: &'static str = "ShareService";
}

impl ShareService {
    pub fn new(rooms: RoomRepository) -> Self {
        Self { rooms }
    }

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
        &self,
        client_id: Uuid,
        name_filter: Option<String>,
        cursor: ShareTargetCursor,
        page_size: usize,
    ) -> Result<CursorResults<ShareTargetResponse>, AppError> {
        let name = name_filter.as_deref();
        let mut content: Vec<ShareTargetResponse> = Vec::with_capacity(page_size);

        // ── Phase 1: active section (rooms with recent activity) ──────────────
        if cursor.phase == SharePhase::Active {
            let mut rows = self
                .rooms
                .active_share_targets(&client_id, name, cursor.last_active_at, cursor.last_id, (page_size + 1) as i64)
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
                content.extend(rows.into_iter().map(ShareTargetResponse::from));
                return Self::encode(content, next);
            }

            // Active section fits entirely on this page.
            content.extend(rows.into_iter().map(ShareTargetResponse::from));

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

        let mut rows = self
            .rooms
            .inactive_share_targets(&client_id, name, cursor_name, cursor_id, (remaining + 1) as i64)
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

        content.extend(rows.into_iter().map(ShareTargetResponse::from));
        Self::encode(content, next)
    }

    fn encode(content: Vec<ShareTargetResponse>, next: Option<ShareTargetCursor>) -> Result<CursorResults<ShareTargetResponse>, AppError> {
        let cursor = match next {
            Some(c) => Some(encode_cursor(&c).map_err(|e| AppError::Processing(format!("Cursor encoding failed: {e}")))?),
            None => None,
        };
        Ok(CursorResults { cursor, content })
    }
}
