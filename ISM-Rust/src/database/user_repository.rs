use async_trait::async_trait;
use chrono::Utc;
use log::{error, info};
use sqlx::{Pool, Postgres, QueryBuilder};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;
use crate::core::{UserDbConfig};
use crate::database::user::User;
use crate::model::{ChatRoomEntity, NewRoom, RoomType};

#[derive(Debug, Clone)]
pub struct PgDbClient {
    pool: Pool<Postgres>,
}

impl PgDbClient {
    pub fn new(pool: Pool<Postgres>) -> Self {
        PgDbClient { pool }
    }
}

#[async_trait]
pub trait RoomRepository {

    async fn select_all_user_in_room(&self, room_id: &Uuid) -> Result<Vec<User>, sqlx::Error>;
    async fn get_joined_rooms(&self, user_id: &Uuid) -> Result<Vec<ChatRoomEntity>, sqlx::Error>;
    async fn insert_room(&self, room: NewRoom) -> Result<ChatRoomEntity, sqlx::Error>;
    async fn select_room(&self, room_id: &Uuid) -> Result<ChatRoomEntity, sqlx::Error>;
    async fn is_user_in_room(&self, user_id: &Uuid, room_id: &Uuid) -> Result<bool, sqlx::Error>;
    async fn select_room_participants_ids(&self, room_id: &Uuid) -> Result<Vec<Uuid>, sqlx::Error>;
}

#[async_trait]
impl RoomRepository for PgDbClient {


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

    async fn get_joined_rooms(&self, user_id: &Uuid) -> Result<Vec<ChatRoomEntity>, sqlx::Error> {
        let rooms = sqlx::query_as!(ChatRoomEntity,
            r#"
                SELECT room.id, room.room_type as "room_type: RoomType", room.room_name, room.created_at
                FROM chat_room_participant AS participants
                JOIN chat_room AS room ON participants.room_id = room.id
                WHERE participants.user_id = $1
            "#, user_id).fetch_all(&self.pool).await?;
        Ok(rooms)
    }

    async fn insert_room(&self, new_room: NewRoom) -> Result<ChatRoomEntity, sqlx::Error> {
        let room_entity = ChatRoomEntity {
            id: Uuid::new_v4(),
            room_type: new_room.room_type,
            room_name: new_room.room_name.unwrap_or_else(|| String::from("Neuer Chat")),
            created_at: Utc::now()
        };

        //https://docs.rs/sqlx/latest/sqlx/struct.Transaction.html
        let mut tx = self.pool.begin().await?;

        let room = sqlx::query_as!(
            ChatRoomEntity,
            r#"
            INSERT INTO chat_room (id, room_type, room_name, created_at)
            VALUES ($1, $2, $3, $4)
            RETURNING id, room_name, created_at, room_type as "room_type: RoomType"
            "#,
            &room_entity.id,
            &room_entity.room_type.to_string(),
            &room_entity.room_name,
            &room_entity.created_at
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
            SELECT id, room_type as "room_type: RoomType", room_name, created_at
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

}

pub async fn init_room_db(config: &UserDbConfig) -> PgDbClient {
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
            info!("Established connection to the user database.");
            pool
        }
        Err(err) => {
            error!("Failed to connect to the user database: {:?}", err);
            std::process::exit(1);
        }
    };
    PgDbClient::new(pool)
}