use async_trait::async_trait;
use chrono::Utc;
use log::{error, info};
use sqlx::{Pool, Postgres, QueryBuilder};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;
use crate::core::{UserDbConfig};
use crate::model::user::User;
use crate::model::{ChatRoomEntity, ChatRoomListItemDTO, Message, NewRoom, RoomType};

#[derive(Debug, Clone)]
pub struct RoomDatabaseClient {
    pool: Pool<Postgres>,
}

impl RoomDatabaseClient {

    pub async fn new(config: &UserDbConfig) -> Self {
        let opt = PgConnectOptions::new()
            .host(&config.db_host)
            .port(config.db_port)
            .database(&config.db_name)
            .username(&config.db_user)
            .password(&config.db_password);
        let pool = match PgPoolOptions::new()
            .max_connections(10)
            .connect_with(opt)
            .await
        {
            Ok(pool) => {
                info!("Established connection to the room database.");
                pool
            }
            Err(err) => {
                error!("Failed to connect to the room database: {:?}", err);
                std::process::exit(1);
            }
        };
        RoomDatabaseClient { pool }
    }
}

#[async_trait]
pub trait RoomRepository {

    async fn select_all_user_in_room(&self, room_id: &Uuid) -> Result<Vec<User>, sqlx::Error>;
    async fn get_joined_rooms(&self, user_id: &Uuid) -> Result<Vec<ChatRoomListItemDTO>, sqlx::Error>;
    async fn find_specific_joined_room(&self, room_id: &Uuid, user_id: &Uuid) -> Result<Option<ChatRoomListItemDTO>, sqlx::Error>;
    async fn insert_room(&self, room: NewRoom) -> Result<ChatRoomEntity, sqlx::Error>;
    async fn select_room(&self, room_id: &Uuid) -> Result<ChatRoomEntity, sqlx::Error>;
    async fn is_user_in_room(&self, user_id: &Uuid, room_id: &Uuid) -> Result<bool, sqlx::Error>;
    async fn select_room_participants_ids(&self, room_id: &Uuid) -> Result<Vec<Uuid>, sqlx::Error>;
    async fn update_last_room_message(&self, room_id: &Uuid, text: &Message) -> Result<String, sqlx::Error>;
    async fn update_user_read_status(&self, room_id: &Uuid, user_id: &Uuid) -> Result<(), sqlx::Error>;
}


#[async_trait]
impl RoomRepository for RoomDatabaseClient {

    async fn select_all_user_in_room(&self, room_id: &Uuid) -> Result<Vec<User>, sqlx::Error> {
        let users = sqlx::query_as!(User,
            r#"
            SELECT users.id, users.display_name, users.profile_picture,
            participants.room_id, participants.joined_at, participants.last_message_read_at
            FROM chat_room_participant AS participants
            JOIN app_user AS users ON participants.user_id = users.id
            WHERE participants.room_id = $1
            "#, room_id).fetch_all(&self.pool).await?;
        Ok(users)
    }

    async fn get_joined_rooms(&self, user_id: &Uuid) -> Result<Vec<ChatRoomListItemDTO>, sqlx::Error> {
        let rooms = sqlx::query_as!(
            ChatRoomListItemDTO,
            r#"
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
            WHERE participants.user_id = $1
            "#,
            user_id
        ).fetch_all(&self.pool).await?;
        Ok(rooms)
    }

    async fn find_specific_joined_room(&self, room_id: &Uuid, user_id: &Uuid) -> Result<Option<ChatRoomListItemDTO>, sqlx::Error> {
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
            WHERE participants.user_id = $1 AND room.id = $2
            "#,
            user_id,
            room_id
        ).fetch_optional(&self.pool).await?;
        Ok(room)
    }

    async fn insert_room(&self, new_room: NewRoom) -> Result<ChatRoomEntity, sqlx::Error> {
        let room_entity = ChatRoomEntity {
            id: Uuid::new_v4(),
            room_type: new_room.room_type,
            room_name: Option::from(new_room.room_name.unwrap_or_else(|| String::from("Neuer Chat"))),
            created_at: Utc::now(),
            latest_message: None
        };

        //https://docs.rs/sqlx/latest/sqlx/struct.Transaction.html
        let mut tx = self.pool.begin().await?;

        let room = sqlx::query_as!(
            ChatRoomEntity,
            r#"
            INSERT INTO chat_room (id, room_type, room_name, created_at, latest_message)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id, room_name, created_at, room_type as "room_type: RoomType", latest_message
            "#,
            &room_entity.id,
            &room_entity.room_type.to_string(),
            room_entity.room_name,
            &room_entity.created_at,
            room_entity.latest_message
        ).fetch_one(&mut *tx).await?;

        //https://docs.rs/sqlx-core/0.5.13/sqlx_core/query_builder/struct.QueryBuilder.html#method.push_values
        let mut builder: QueryBuilder<Postgres> = QueryBuilder::new(
            "INSERT INTO chat_room_participant (user_id, room_id, joined_at) "
        );
        builder.push_values(&new_room.invited_users, |mut db, user| {
            db.push_bind(user)
              .push_bind(&room.id)
              .push_bind(Utc::now());
        }).build().fetch_all(&mut *tx).await?;

        tx.commit().await?;
        Ok(room)
    }

    async fn select_room(&self, room_id: &Uuid) -> Result<ChatRoomEntity, sqlx::Error> {
        let room_details = sqlx::query_as!(
            ChatRoomEntity,
            r#"
            SELECT id, room_type as "room_type: RoomType", room_name, created_at, latest_message
            FROM chat_room
            WHERE id = $1
            "#, room_id).fetch_one(&self.pool).await?;
        Ok(room_details)
    }

    async fn is_user_in_room(&self, user_id: &Uuid, room_id: &Uuid) -> Result<bool, sqlx::Error> {
        let exists = sqlx::query_scalar!(
            r#"
            SELECT EXISTS(
                SELECT 1
                FROM chat_room_participant
                WHERE user_id = $1 AND room_id = $2
                )
        "#, user_id, room_id).fetch_one(&self.pool).await?;
        Ok(exists.unwrap_or(false))
    }

    async fn select_room_participants_ids(&self, room_id: &Uuid) -> Result<Vec<Uuid>, sqlx::Error> {
        let result = sqlx::query!(r#"SELECT user_id FROM chat_room_participant WHERE room_id = $1"#, room_id).fetch_all(&self.pool).await?;
        let user: Vec<Uuid> = result.iter().map(|id| id.user_id).collect();
        Ok(user)
    }

    async fn update_last_room_message(&self, room_id: &Uuid, msg: &Message) -> Result<String, sqlx::Error> {
        let name = sqlx::query!("SELECT display_name FROM app_user WHERE id = $1", &msg.sender_id).fetch_one(&self.pool).await?;
        let preview_text = format!("{}: {}", name.display_name, msg.msg_body);
        sqlx::query!(
            "UPDATE chat_room SET latest_message = NOW(), latest_message_preview_text = $2 WHERE id = $1",
            room_id,
            &preview_text
        ).execute(&self.pool).await?;
        Ok(preview_text)
    }

    async fn update_user_read_status(&self, room_id: &Uuid, user_id: &Uuid) -> Result<(), sqlx::Error> {
        sqlx::query!("Update chat_room_participant SET last_message_read_at = NOW() WHERE user_id = $1 AND room_id = $2", user_id, room_id).execute(&self.pool).await?;
        Ok(())
    }

}