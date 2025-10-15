use chrono::Utc;
use log::{info};
use sqlx::{Error, PgConnection, Pool, Postgres, QueryBuilder, Transaction};
use sqlx::error::BoxDynError;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;
use crate::core::{UserDbConfig};
use crate::model::user::{User, MembershipStatus};
use crate::model::{ChatRoomEntity, ChatRoomListItemDTO, NewRoom, RoomType};

#[derive(Debug, Clone)]
pub struct RoomDatabase {
    pool: Pool<Postgres>,
}

impl RoomDatabase {

    pub async fn new(config: &UserDbConfig) -> Self {
        let opt = PgConnectOptions::new()
            .host(&config.db_host)
            .port(config.db_port)
            .database(&config.db_name)
            .username(&config.db_user)
            .password(&config.db_password);
        let pool = match PgPoolOptions::new()
            .max_connections(25)
            .connect_with(opt)
            .await
        {
            Ok(pool) => {
                info!("Established connection to the room database.");
                pool
            }
            Err(err) => {
                panic!("Failed to connect to the room database: {:?}", err);
            }
        };
        RoomDatabase { pool }
    }

    pub async fn start_transaction(&self) -> Result<Transaction<Postgres>, Error> {
        let tx = self.pool.begin().await?;
        Ok(tx)
    }

    pub fn get_connection(&self) -> &Pool<Postgres> {
        &self.pool
    }

    pub async fn select_all_user_in_room(&self, room_id: &Uuid) -> Result<Vec<User>, sqlx::Error> {
        let users = sqlx::query_as!(User,
            r#"
            SELECT users.id,
                   users.display_name,
                   users.profile_picture,
                   participants.joined_at,
                   participants.last_message_read_at,
                   participants.participant_state AS "membership_status: MembershipStatus"
            FROM chat_room_participant AS participants
            JOIN app_user AS users ON participants.user_id = users.id
            WHERE participants.room_id = $1
            "#, room_id).fetch_all(&self.pool).await?;
        Ok(users)
    }

    pub async fn select_joined_user_in_room(&self, room_id: &Uuid) -> Result<Vec<User>, sqlx::Error> {
        let users = sqlx::query_as!(User,
            r#"
            SELECT
                users.id,
                users.display_name,
                users.profile_picture,
                participants.joined_at,
                participants.last_message_read_at,
                participants.participant_state AS "membership_status: MembershipStatus"
            FROM chat_room_participant AS participants
            JOIN app_user AS users ON participants.user_id = users.id
            WHERE participants.room_id = $1 AND participants.participant_state = 'Joined'
            "#, room_id).fetch_all(&self.pool).await?;
        Ok(users)
    }

    pub async fn get_joined_rooms(&self, user_id: &Uuid) -> Result<Vec<ChatRoomListItemDTO>, sqlx::Error> {
        let rooms = sqlx::query_as!(
            ChatRoomListItemDTO,
            r#"
            WITH room_selection AS (
                SELECT DISTINCT ON (room.id)
                    room.id,
                    room.room_type AS "room_type: RoomType",
                    room.created_at,
                    room.latest_message,
                    room.latest_message_preview_text,
                    CASE
                        WHEN room.room_type = 'Single' THEN u.display_name
                        ELSE room.room_name
                    END AS room_name,
                    CASE
                        WHEN room.room_type = 'Single' THEN u.profile_picture
                        ELSE room.room_image_url
                    END AS room_image_url,
                    CASE
                        WHEN participants.last_message_read_at < room.latest_message THEN TRUE
                        ELSE FALSE
                    END AS unread
                FROM chat_room_participant AS participants
                JOIN chat_room AS room ON participants.room_id = room.id
                LEFT JOIN chat_room_participant crp ON crp.room_id = room.id AND crp.user_id != $1
                LEFT JOIN app_user u ON u.id = crp.user_id
                WHERE participants.user_id = $1 AND participants.participant_state = 'Joined'
            )
            SELECT * FROM room_selection
            ORDER BY latest_message DESC
            "#,
            user_id
        ).fetch_all(&self.pool).await?;
        Ok(rooms)
    }

    pub async fn delete_room(&self, conn: &mut PgConnection, room_id: &Uuid) -> Result<(), sqlx::Error> {
        sqlx::query!("DELETE FROM chat_room_participant WHERE room_id = $1", room_id).execute(&mut *conn).await?;
        sqlx::query!("DELETE FROM chat_room WHERE id = $1",room_id).execute(&mut *conn).await?;
        Ok(())
    }

    pub async fn find_specific_joined_room(&self, room_id: &Uuid, user_id: &Uuid) -> Result<Option<ChatRoomListItemDTO>, sqlx::Error> {
        let room = sqlx::query_as!(
            ChatRoomListItemDTO,
            r#"
            SELECT
                room.id,
                room.room_type AS "room_type: RoomType",
                room.created_at,
                room.latest_message,
                room.latest_message_preview_text,
                CASE
                    WHEN room.room_type = 'Single' THEN u.display_name
                    ELSE room.room_name
                END AS room_name,
                CASE
                    WHEN room.room_type = 'Single' THEN u.profile_picture
                    ELSE room.room_image_url
                END AS room_image_url,
                CASE
                    WHEN participants.last_message_read_at < room.latest_message THEN TRUE
                    ELSE FALSE
                END AS unread
            FROM chat_room_participant AS participants
            JOIN chat_room AS room ON participants.room_id = room.id
            LEFT JOIN chat_room_participant crp ON crp.room_id = room.id AND crp.user_id != $1
            LEFT JOIN app_user u ON u.id = crp.user_id
            WHERE participants.user_id = $1 AND room.id = $2 AND participants.participant_state = 'Joined'
            "#,
            user_id,
            room_id
        ).fetch_optional(&self.pool).await?;
        Ok(room)
    }

    pub async fn insert_room(&self, new_room: NewRoom) -> Result<ChatRoomEntity, sqlx::Error> {
        let room_entity = ChatRoomEntity {
            id: Uuid::new_v4(),
            room_type: new_room.room_type,
            room_name: new_room.room_name,
            room_image_url: None,
            created_at: Utc::now(),
            latest_message: Option::from(Utc::now()),
            latest_message_preview_text: Option::from(String::from("Chat wurde erstellt.")),
        };

        //https://docs.rs/sqlx/latest/sqlx/struct.Transaction.html
        let mut tx = self.pool.begin().await?;

        let room = sqlx::query_as!(
            ChatRoomEntity,
            r#"
            INSERT INTO chat_room (id, room_type, room_name, created_at, latest_message, latest_message_preview_text)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id, room_name, created_at, room_type as "room_type: RoomType", latest_message, latest_message_preview_text, room_image_url
            "#,
            room_entity.id,
            room_entity.room_type.to_string(),
            room_entity.room_name,
            room_entity.created_at,
            room_entity.latest_message,
            room_entity.latest_message_preview_text
        ).fetch_one(&mut *tx).await?;

        //https://docs.rs/sqlx-core/0.5.13/sqlx_core/query_builder/struct.QueryBuilder.html#method.push_values
        let mut builder: QueryBuilder<Postgres> = QueryBuilder::new(
            "INSERT INTO chat_room_participant (user_id, room_id, joined_at, participant_state) "
        );
        builder.push_values(&new_room.invited_users, |mut db, user| {
            db.push_bind(user)
                .push_bind(&room.id)
                .push_bind(Utc::now())
                .push_bind(MembershipStatus::Joined.to_string());
        }).build().fetch_all(&mut *tx).await?;

        tx.commit().await?;
        Ok(room)
    }

    pub async fn select_room(&self, room_id: &Uuid) -> Result<ChatRoomEntity, sqlx::Error> {
        let room_details = sqlx::query_as!(
            ChatRoomEntity,
            r#"
            SELECT id, room_type as "room_type: RoomType", room_name, created_at, latest_message, room_image_url, latest_message_preview_text
            FROM chat_room
            WHERE id = $1
            "#, room_id).fetch_one(&self.pool).await?;
        Ok(room_details)
    }

    pub async fn is_user_in_room(&self, user_id: &Uuid, room_id: &Uuid) -> Result<bool, sqlx::Error> {
        let exists = sqlx::query_scalar!(
            r#"
            SELECT EXISTS(
                SELECT 1
                FROM chat_room_participant
                WHERE user_id = $1 AND room_id = $2 AND participant_state = 'Joined'
            )
        "#, user_id, room_id).fetch_one(&self.pool).await?;
        Ok(exists.unwrap_or(false))
    }

    pub async fn find_room_between_users(&self, user_id: &Uuid, other_user_id: &Uuid) -> Result<Option<Uuid>, sqlx::Error> {
        let room_details = sqlx::query!(
            r#"
            SELECT r.id
            FROM chat_room r
                JOIN chat_room_participant p ON r.id = p.room_id
            WHERE r.room_type = 'Single' AND p.user_id IN ($1, $2) AND p.participant_state = 'Joined'
            GROUP BY r.id
            HAVING COUNT(p.user_id) = 2
            "#, user_id, other_user_id).fetch_optional(&self.pool).await?;

        match room_details {
            Some(room) => Ok(Some(room.id)),
            None => Ok(None)
        }
    }

    pub async fn add_user_to_room(&self, user_id: &Uuid, room_id: &Uuid) -> Result<User, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query!("INSERT INTO chat_room_participant (user_id, room_id, joined_at) VALUES ($1, $2, $3) ON CONFLICT (user_id, room_id) DO UPDATE SET joined_at = $3, participant_state = 'Joined'",
            user_id, room_id, Utc::now()).execute(&mut *tx).await?;

        let user = sqlx::query_as!(User,
           r#"
            SELECT
                users.id,
                users.display_name,
                users.profile_picture,
                participants.joined_at,
                participants.last_message_read_at,
                participants.participant_state AS "membership_status: MembershipStatus"
            FROM chat_room_participant AS participants
            JOIN app_user AS users ON participants.user_id = users.id
            WHERE participants.user_id = $1 AND participants.room_id = $2
            "#, user_id, room_id).fetch_one(&mut *tx).await?;
        let text = format!("{}{}", user.display_name, String::from(" ist in dem Chat beigetreten.")); //todo: think about a better latest msg logic
        sqlx::query!("UPDATE chat_room SET latest_message = NOW(), latest_message_preview_text = $2 WHERE id = $1", room_id, text).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(user)
    }

    pub async fn select_room_participants_ids(&self, room_id: &Uuid) -> Result<Vec<Uuid>, sqlx::Error> {
        let result = sqlx::query!(r#"SELECT user_id FROM chat_room_participant WHERE room_id = $1 AND participant_state = 'Joined'"#, room_id).fetch_all(&self.pool).await?;
        let user: Vec<Uuid> = result.iter().map(|id| id.user_id).collect();
        Ok(user)
    }

    /// If you really just want to accept both, a transaction or a
    /// connection as an argument to a function, then it's easier to just accept a
    /// mutable reference to a database connection like so:
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
    pub async fn update_last_room_message(&self, conn: &mut PgConnection, room_id: &Uuid, sender_id: &Uuid, preview_text: String) -> Result<String, BoxDynError>
    {
        let name = sqlx::query!("SELECT display_name FROM app_user WHERE id = $1", sender_id).fetch_one(&mut *conn).await?;
        let text = format!("{}{}", name.display_name, preview_text);
        sqlx::query!(
            "UPDATE chat_room SET latest_message = NOW(), latest_message_preview_text = $2 WHERE id = $1",
            room_id,
            &text
        ).execute(&mut *conn).await?;
        Ok(text)
    }

    pub async fn update_user_read_status<'e, E>(&self, exec: E, room_id: &Uuid, user_id: &Uuid) -> Result<(), sqlx::Error>
    where E: sqlx::Executor<'e, Database = Postgres>
    {
        sqlx::query!("Update chat_room_participant SET last_message_read_at = NOW() WHERE user_id = $1 AND room_id = $2", user_id, room_id).execute(exec).await?;
        Ok(())
    }

    pub async fn update_room_img_url(&self, room_id: &Uuid, image_url: &String) -> Result<(), sqlx::Error> {
        sqlx::query!("UPDATE chat_room SET room_image_url = $1 WHERE id = $2", image_url, room_id).execute(&self.pool).await?;
        Ok(())
    }


    pub async fn remove_user_from_room(&self, conn: &mut PgConnection, room_id: &Uuid, user: &User) -> Result<(), sqlx::Error> {
        sqlx::query!("UPDATE chat_room_participant SET participant_state = 'Left' WHERE user_id = $1 AND room_id = $2", user.id, room_id).execute(&mut *conn).await?;
        let text = format!("{}{}", user.display_name, String::from(" hat den Chat verlassen.")); //todo: think about a better latest msg logic
        sqlx::query!("UPDATE chat_room SET latest_message = NOW(), latest_message_preview_text = $2 WHERE id = $1", room_id, text).execute(&mut *conn).await?;
        Ok(())
    }

}