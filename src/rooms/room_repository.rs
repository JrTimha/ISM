use crate::rooms::room::{
    ChatRoomEntity, LastMessagePreviewText, NewRoom, RoomPaginationCursor, RoomType,
};
use crate::rooms::room_member::RoomMember;
use crate::rooms::share_service::{ActiveShareRow, InactiveShareRow};
use chrono::{DateTime, Utc};
use sqlx::types::Json;
use sqlx::{Error, PgConnection, Pool, Postgres, QueryBuilder, Transaction};
use uuid::Uuid;

#[derive(Clone)]
pub struct RoomRepository {
    pool: Pool<Postgres>,
}

impl RoomRepository {
    pub fn new(pool: Pool<Postgres>) -> Self {
        RoomRepository { pool }
    }

    pub async fn start_transaction(&self) -> Result<Transaction<'_, Postgres>, Error> {
        let tx = self.pool.begin().await?;
        Ok(tx)
    }

    pub fn get_connection(&self) -> &Pool<Postgres> {
        &self.pool
    }

    pub async fn select_all_room_member(
        &self,
        room_id: &Uuid,
    ) -> Result<Vec<RoomMember>, sqlx::Error> {
        let users = sqlx::query_as!(
            RoomMember,
            r#"
            SELECT users.id,
                   users.display_name,
                   users.profile_picture,
                   participants.joined_at AS "joined_at?",
                   participants.last_message_read_at
            FROM chat_room_participant AS participants
            JOIN app_user AS users ON participants.user_id = users.id
            WHERE participants.room_id = $1
            "#,
            room_id
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(users)
    }

    /// Paginated list of a user's joined rooms, ordered by recent activity.
    ///
    /// - `name_filter`: optional case-insensitive substring match. For single rooms
    ///   this matches the other participant's display name, for groups the room name
    ///   (the same `COALESCE` that produces `room_name` in the result).
    /// - Keyset over `(latest_message, id)` so paging is stable under inserts.
    ///   Callers pass `limit = page_size + 1` to detect a following page.
    ///
    /// Uses a runtime query (not the `query_as!` macro) because of the optional
    /// cursor/name binds — consistent with the relationship queries in `UserRepository`.
    pub async fn get_joined_rooms(
        &self,
        user_id: &Uuid,
        name_filter: Option<&str>,
        cursor: RoomPaginationCursor,
        limit: i64,
    ) -> Result<Vec<ChatRoomEntity>, sqlx::Error> {
        let rooms = sqlx::query_as!(
            ChatRoomEntity,
            r#"
            SELECT
                room.id,
                room.room_type AS "room_type: RoomType",
                room.created_at,
                room.latest_message,
                room.latest_message_preview_text AS "latest_message_preview_text: Json<LastMessagePreviewText>",
                COALESCE(other_user.display_name, room.room_name) AS room_name,
                COALESCE(other_user.profile_picture, room.room_image_url) AS room_image_url,
                COALESCE(p1.last_message_read_at < room.latest_message, TRUE) AS unread
            FROM
                chat_room_participant AS p1
            JOIN
                chat_room AS room ON p1.room_id = room.id
            -- To find the other participant, only for single chat rooms!
            LEFT JOIN LATERAL (
                SELECT
                    p2.user_id
                FROM
                    chat_room_participant p2
                WHERE
                    p2.room_id = room.id AND p2.user_id != $1
                -- Only take the first match
                LIMIT 1
            ) AS other_participant ON room.room_type = 'Single'
            -- Only executed when the lateral join has matched something:
            LEFT JOIN
                app_user AS other_user ON other_user.id = other_participant.user_id
            WHERE
                p1.user_id = $1
                AND ($2::text IS NULL OR COALESCE(other_user.display_name, room.room_name) ILIKE concat('%', $2, '%'))
                AND (
                    $3::timestamptz IS NULL
                    OR room.latest_message < $3
                    OR (room.latest_message = $3 AND room.id < $4)
                )
            ORDER BY
                room.latest_message DESC, room.id DESC
            LIMIT $5
            "#,
            user_id,
            name_filter,
            cursor.last_seen_latest_message,
            cursor.last_seen_room_id,
            limit
        ).fetch_all(&self.pool).await?;
        Ok(rooms)
    }

    /// *Active* section of the share-target list: group rooms the client is in, plus
    /// friends with whom an 1-1 room already exists, merged and ordered by recent
    /// activity (`active_at DESC`, `room_id` tie-breaker). Friends without a 1-1 room
    /// are excluded here — they belong to the inactive section (`inactive_share_targets`),
    /// so the two halves of the friend set never overlap.
    ///
    /// Optional case-insensitive name filter (friend `raw_name`, group `room_name`).
    /// Keyset over `(active_at, room_id)`; callers pass `limit = page_size + 1`.
    ///
    /// Runtime query (not the `query_as!` macro) because of the optional cursor/name
    /// binds — consistent with `get_joined_rooms` and the `UserRepository` queries.
    pub async fn active_share_targets(
        &self,
        client_id: &Uuid,
        name_filter: Option<&str>,
        cursor_active_at: Option<DateTime<Utc>>,
        cursor_id: Option<Uuid>,
        limit: i64,
    ) -> Result<Vec<ActiveShareRow>, sqlx::Error> {
        let rows = sqlx::query_as::<_, ActiveShareRow>(
            r#"
            SELECT name, room_id, image_url, active_at, is_group, user_id
            FROM (
                -- Friends with an existing 1-1 room (the share target is that room).
                SELECT
                    u.display_name AS name,
                    sr.room_id     AS room_id,
                    u.profile_picture AS image_url,
                    sr.active_at   AS active_at,
                    false          AS is_group,
                    u.id           AS user_id
                FROM app_user u
                JOIN user_relationship rl
                    ON u.id = CASE
                                WHEN rl.user_a_id = $1 THEN rl.user_b_id
                                WHEN rl.user_b_id = $1 THEN rl.user_a_id
                              END
                   AND rl.state = 'FRIEND'
                JOIN LATERAL (
                    SELECT r.id AS room_id,
                           COALESCE(r.latest_message, r.created_at) AS active_at
                    FROM chat_room r
                    JOIN chat_room_participant p1 ON p1.room_id = r.id AND p1.user_id = $1
                    JOIN chat_room_participant p2 ON p2.room_id = r.id AND p2.user_id = u.id
                    WHERE r.room_type = 'Single'
                    LIMIT 1
                ) sr ON true
                WHERE ($2::text IS NULL OR u.raw_name LIKE lower(concat('%', $2, '%')))

                UNION ALL

                -- Group rooms the client is a member of.
                SELECT
                    r.room_name      AS name,
                    r.id             AS room_id,
                    r.room_image_url AS image_url,
                    COALESCE(r.latest_message, r.created_at) AS active_at,
                    true             AS is_group,
                    NULL::uuid       AS user_id
                FROM chat_room r
                JOIN chat_room_participant p ON p.room_id = r.id AND p.user_id = $1
                WHERE r.room_type = 'Group'
                  AND ($2::text IS NULL OR r.room_name ILIKE concat('%', $2, '%'))
            ) AS merged
            WHERE (
                $3::timestamptz IS NULL
                OR active_at < $3
                OR (active_at = $3 AND room_id < $4)
            )
            ORDER BY active_at DESC, room_id DESC
            LIMIT $5
            "#,
        )
        .bind(client_id)
        .bind(name_filter)
        .bind(cursor_active_at)
        .bind(cursor_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// *Inactive* section of the share-target list: friends the client has no 1-1 room
    /// with yet (sharing requires creating the room first). Ordered alphabetically
    /// (`display_name ASC`, `id` tie-breaker), keyset over `(display_name, id)`.
    ///
    /// The `NOT EXISTS` is the exact complement of the 1-1-room join in
    /// `active_share_targets`, so every friend appears in exactly one of the two sections.
    pub async fn inactive_share_targets(
        &self,
        client_id: &Uuid,
        name_filter: Option<&str>,
        cursor_name: Option<String>,
        cursor_id: Option<Uuid>,
        limit: i64,
    ) -> Result<Vec<InactiveShareRow>, sqlx::Error> {
        let rows = sqlx::query_as::<_, InactiveShareRow>(
            r#"
            SELECT u.display_name AS name, u.id AS user_id, u.profile_picture AS image_url
            FROM app_user u
            JOIN user_relationship rl
                ON u.id = CASE
                            WHEN rl.user_a_id = $1 THEN rl.user_b_id
                            WHEN rl.user_b_id = $1 THEN rl.user_a_id
                          END
               AND rl.state = 'FRIEND'
            WHERE ($2::text IS NULL OR u.raw_name LIKE lower(concat('%', $2, '%')))
              AND NOT EXISTS (
                  SELECT 1
                  FROM chat_room r
                  JOIN chat_room_participant p1 ON p1.room_id = r.id AND p1.user_id = $1
                  JOIN chat_room_participant p2 ON p2.room_id = r.id AND p2.user_id = u.id
                  WHERE r.room_type = 'Single'
              )
              AND ($3::text IS NULL OR (u.display_name, u.id) > ($3, $4))
            ORDER BY u.display_name ASC, u.id ASC
            LIMIT $5
            "#,
        )
        .bind(client_id)
        .bind(name_filter)
        .bind(cursor_name)
        .bind(cursor_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn delete_room(
        &self,
        conn: &mut PgConnection,
        room_id: &Uuid,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            "DELETE FROM chat_room_participant WHERE room_id = $1",
            room_id
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query!("DELETE FROM chat_room WHERE id = $1", room_id)
            .execute(&mut *conn)
            .await?;
        Ok(())
    }

    pub async fn find_specific_joined_room(
        &self,
        room_id: &Uuid,
        user_id: &Uuid,
    ) -> Result<Option<ChatRoomEntity>, sqlx::Error> {
        let room = sqlx::query_as!(
            ChatRoomEntity,
            r#"
            SELECT
                room.id,
                room.room_type AS "room_type: RoomType",
                room.created_at,
                room.latest_message,
                room.latest_message_preview_text AS "latest_message_preview_text: Json<LastMessagePreviewText>",
                COALESCE(other_user.display_name, room.room_name) AS room_name,
                COALESCE(other_user.profile_picture, room.room_image_url) AS room_image_url,
                COALESCE(participants.last_message_read_at < room.latest_message, TRUE) AS unread
            FROM
                chat_room_participant AS participants
            JOIN
                chat_room AS room ON participants.room_id = room.id
            -- 3. To find the other participant, only for single chat rooms!
            LEFT JOIN LATERAL (
                SELECT
                    p2.user_id
                FROM
                    chat_room_participant p2
                WHERE
                    p2.room_id = room.id AND p2.user_id != $1
                LIMIT 1
            ) AS other_participant ON room.room_type = 'Single'
            -- Only executed when the lateral join has matched something:
            LEFT JOIN
                app_user AS other_user ON other_user.id = other_participant.user_id
            WHERE
                participants.user_id = $1
                AND room.id = $2
            "#,
            user_id,
            room_id
        ).fetch_optional(&self.pool).await?;
        Ok(room)
    }

    /// Inserts the room row and its participants on the given connection. The caller
    /// owns the transaction so room creation can be made atomic together with an
    /// optional first message (see `RoomService::create_room`).
    pub async fn insert_room(
        &self,
        conn: &mut PgConnection,
        new_room: &NewRoom,
    ) -> Result<ChatRoomEntity, sqlx::Error> {
        let room_entity = ChatRoomEntity {
            id: Uuid::new_v4(),
            room_type: new_room.room_type.clone(),
            room_name: new_room.room_name.clone(),
            room_image_url: None,
            created_at: Utc::now(),
            latest_message: Some(Utc::now()),
            latest_message_preview_text: Some(Json(LastMessagePreviewText::New)),
            unread: None,
        };

        let room = sqlx::query_as!(
            ChatRoomEntity,
            r#"
            INSERT INTO chat_room (id, room_type, room_name, created_at, latest_message, latest_message_preview_text)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id, room_name, created_at, room_type as "room_type: RoomType", latest_message, latest_message_preview_text AS "latest_message_preview_text: Json<LastMessagePreviewText>", room_image_url, TRUE as "unread: _"
            "#,
            room_entity.id,
            room_entity.room_type.to_string(),
            room_entity.room_name,
            room_entity.created_at,
            room_entity.latest_message,
            room_entity.latest_message_preview_text as Option<Json<LastMessagePreviewText>>
        ).fetch_one(&mut *conn).await?;

        //https://docs.rs/sqlx-core/0.5.13/sqlx_core/query_builder/struct.QueryBuilder.html#method.push_values
        let mut builder: QueryBuilder<Postgres> =
            QueryBuilder::new("INSERT INTO chat_room_participant (user_id, room_id, joined_at) ");
        builder
            .push_values(&new_room.invited_users, |mut db, user| {
                db.push_bind(user).push_bind(&room.id).push_bind(Utc::now());
            })
            .build()
            .execute(&mut *conn)
            .await?;

        Ok(room)
    }

    pub async fn select_room(&self, room_id: &Uuid) -> Result<ChatRoomEntity, sqlx::Error> {
        let room_details = sqlx::query_as!(
            ChatRoomEntity,
            r#"
            SELECT
                id,
                room_type as "room_type: RoomType",
                room_name,
                created_at,
                latest_message,
                room_image_url,
                latest_message_preview_text AS "latest_message_preview_text: Json<LastMessagePreviewText>",
                NULL::boolean as "unread: _"
            FROM chat_room
            WHERE id = $1
            "#, room_id).fetch_one(&self.pool).await?;
        Ok(room_details)
    }

    pub async fn is_user_in_room(
        &self,
        user_id: &Uuid,
        room_id: &Uuid,
    ) -> Result<bool, sqlx::Error> {
        let exists = sqlx::query_scalar!(
            r#"
            SELECT EXISTS(
                SELECT 1
                FROM chat_room_participant
                WHERE user_id = $1 AND room_id = $2
            )
        "#,
            user_id,
            room_id
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(exists.unwrap_or(false))
    }

    pub async fn find_room_between_users(
        &self,
        user_id: &Uuid,
        other_user_id: &Uuid,
    ) -> Result<Option<Uuid>, sqlx::Error> {
        let room_details = sqlx::query!(
            r#"
            SELECT r.id
            FROM chat_room r
                JOIN chat_room_participant p ON r.id = p.room_id
            WHERE r.room_type = 'Single' AND p.user_id IN ($1, $2)
            GROUP BY r.id
            HAVING COUNT(p.user_id) = 2
            "#,
            user_id,
            other_user_id
        )
        .fetch_optional(&self.pool)
        .await?;

        match room_details {
            Some(room) => Ok(Some(room.id)),
            None => Ok(None),
        }
    }

    pub async fn add_user_to_room(
        &self,
        conn: &mut PgConnection,
        user_id: &Uuid,
        room_id: &Uuid,
    ) -> Result<RoomMember, sqlx::Error> {
        sqlx::query!(
            r#"
                INSERT INTO chat_room_participant (user_id, room_id, joined_at)
                VALUES ($1, $2, $3)
                ON CONFLICT (user_id, room_id)
                DO UPDATE SET joined_at = $3
                "#,
            user_id,
            room_id,
            Utc::now()
        )
        .execute(&mut *conn)
        .await?;

        let user = sqlx::query_as!(
            RoomMember,
            r#"
            SELECT
                users.id,
                users.display_name,
                users.profile_picture,
                participants.joined_at AS "joined_at?",
                participants.last_message_read_at
            FROM chat_room_participant AS participants
            JOIN app_user AS users ON participants.user_id = users.id
            WHERE participants.user_id = $1 AND participants.room_id = $2
            "#,
            user_id,
            room_id
        )
        .fetch_one(&mut *conn)
        .await?;
        Ok(user)
    }

    pub async fn select_room_participants_ids(
        &self,
        room_id: &Uuid,
    ) -> Result<Vec<Uuid>, sqlx::Error> {
        let result = sqlx::query!(
            r#"SELECT user_id FROM chat_room_participant WHERE room_id = $1"#,
            room_id
        )
        .fetch_all(&self.pool)
        .await?;
        let user: Vec<Uuid> = result.iter().map(|id| id.user_id).collect();
        Ok(user)
    }

    /// If you really just want to accept both, a transaction or a
    /// connection as an argument to a function, then it's easier to just accept a
    /// mutable reference to a object_storage connection like so:
    ///
    /// ```rust
    /// # use sqlx::{postgres::PgConnection, error::BoxDynError};
    /// # #[cfg(any(postgres_9_6, postgres_14))]
    /// async fn run_query(conn: &mut PgConnection) -> Result<(), BoxDynError> {
    ///     sqlx::query!("SELECT 1 as v").fetch_one(&mut *conn).await?;
    ///     sqlx::query!("SELECT 2 as v").fetch_one(&mut *conn).await?;
    ///
    ///     Ok(())
    /// }
    /// ```
    /// The downside of this approach is that you have to `acquire` a connection
    /// from a pool first and can't directly pass the pool as argument.
    ///
    /// Like this: state.room_repository.get_connection().acquire().await.unwrap();
    ///
    /// [workaround]: https://github.com/launchbadge/sqlx/issues/1015#issuecomment-767787777
    pub async fn update_last_room_message(
        &self,
        conn: &mut PgConnection,
        room_id: &Uuid,
        preview_text: &LastMessagePreviewText,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            "UPDATE chat_room SET latest_message = NOW(), latest_message_preview_text = $2 WHERE id = $1",
            room_id,
            Json(preview_text) as Json<&LastMessagePreviewText>
        ).execute(&mut *conn).await?;
        Ok(())
    }

    pub async fn update_user_read_status<'e, E>(
        &self,
        exec: E,
        room_id: &Uuid,
        user_id: &Uuid,
    ) -> Result<(), sqlx::Error>
    where
        E: sqlx::Executor<'e, Database = Postgres>,
    {
        sqlx::query!("Update chat_room_participant SET last_message_read_at = NOW() WHERE user_id = $1 AND room_id = $2", user_id, room_id).execute(exec).await?;
        Ok(())
    }

    pub async fn update_room_img_url(
        &self,
        room_id: &Uuid,
        image_url: &String,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            "UPDATE chat_room SET room_image_url = $1 WHERE id = $2",
            image_url,
            room_id
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn remove_user_from_room(
        &self,
        conn: &mut PgConnection,
        room_id: &Uuid,
        user_id: &Uuid,
        preview_text: &LastMessagePreviewText,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"
            DELETE FROM chat_room_participant
            WHERE user_id = $1 AND room_id = $2
            "#,
            user_id,
            room_id
        )
        .execute(&mut *conn)
        .await?;

        sqlx::query!(
            r#"
            UPDATE chat_room
                SET latest_message = NOW(),latest_message_preview_text = $2
            WHERE id = $1
            "#,
            room_id,
            Json(preview_text) as Json<&LastMessagePreviewText>
        )
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    /// Resolves the given user ids to `RoomMember`s for a room, used to bundle the
    /// authors of a timeline page. Uses a LEFT JOIN on the participant table so that
    /// senders who have since left the room (no participant row) still resolve from
    /// `app_user`, with `joined_at` / `last_message_read_at` as `None`.
    pub async fn select_message_senders(
        &self,
        room_id: &Uuid,
        sender_ids: &[Uuid],
    ) -> Result<Vec<RoomMember>, sqlx::Error> {
        let senders = sqlx::query_as!(
            RoomMember,
            r#"
            SELECT
                users.id,
                users.display_name,
                users.profile_picture,
                participants.joined_at AS "joined_at?",
                participants.last_message_read_at AS "last_message_read_at?"
            FROM app_user AS users
                LEFT JOIN chat_room_participant AS participants
                    ON participants.user_id = users.id AND participants.room_id = $1
            WHERE users.id = ANY($2)
            "#,
            room_id,
            sender_ids
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(senders)
    }

    /// Atomically updates both the room's latest_message timestamp/preview and
    /// the sender's read status in a single CTE round-trip.
    pub async fn apply_message_to_room(
        &self,
        conn: &mut PgConnection,
        room_id: &Uuid,
        preview_text: &LastMessagePreviewText,
        sender_id: &Uuid,
        timestamp: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query!(
            r#"
            WITH room_update AS (
                UPDATE chat_room
                SET latest_message = $3,
                    latest_message_preview_text = $2
                WHERE id = $1
            )
            UPDATE chat_room_participant
            SET last_message_read_at = $3
            WHERE user_id = $4 AND room_id = $1
            "#,
            room_id,
            Json(preview_text) as Json<&LastMessagePreviewText>,
            timestamp,
            sender_id,
        )
        .execute(&mut *conn)
        .await?;
        Ok(())
    }
}
