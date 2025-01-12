use async_trait::async_trait;
use chrono::Utc;
use log::{error, info};
use sqlx::{FromRow, Pool, Postgres, QueryBuilder};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;
use crate::core::{UserDbConfig};
use crate::database::user::User;
use crate::model::{ChatRoomDetails, ChatRoomEntity, ChatRoomParticipantEntity, NewRoom, RoomType};

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

    async fn get_user(
        &self,
        user_id: Uuid
    ) -> Result<Option<User>, sqlx::Error>;

    async fn insert_room(&self, room: NewRoom) -> Result<(ChatRoomEntity, Vec<ChatRoomParticipantEntity>), sqlx::Error>;
}

#[async_trait]
impl RoomRepository for PgDbClient {

    async fn get_user(&self, user_id: Uuid) -> Result<Option<User>, sqlx::Error> {
        let user = sqlx::query_as!(
                User,
                r#"SELECT id, display_name FROM app_user WHERE id = $1"#,
                user_id
            ).fetch_optional(&self.pool).await?;
        Ok(user)
    }

    async fn insert_room(&self, room: NewRoom) -> Result<(ChatRoomEntity, Vec<ChatRoomParticipantEntity>), sqlx::Error> {
        let room_entity = ChatRoomEntity {
            id: Uuid::new_v4(),
            room_type: room.room_type,
            room_name: room.room_name.unwrap_or_else(|| String::from("Neuer Chat")),
            created_at: Utc::now()
        };
        let participants_entities: Vec<ChatRoomParticipantEntity> = room.invited_users
            .into_iter()
            .map(|user_id| ChatRoomParticipantEntity {
                user_id,
                room_id: room_entity.id,
                joined_at: Utc::now(),
            })
            .collect();
        //https://docs.rs/sqlx/latest/sqlx/struct.Transaction.html
        let mut tx = self.pool.begin().await?;

        let room = sqlx::query_as!(
            ChatRoomEntity,
            r#"
            INSERT INTO chat_room (id, room_type, room_name, created_at)
            VALUES ($1, $2, $3, $4)
            RETURNING id, room_name, created_at, room_type as "room_type: RoomType"
            "#,
            room_entity.id,
            room_entity.room_type.to_string(),
            room_entity.room_name,
            room_entity.created_at
        ).fetch_one(&mut *tx).await?;

        //https://docs.rs/sqlx-core/0.5.13/sqlx_core/query_builder/struct.QueryBuilder.html#method.push_values
        let mut builder: QueryBuilder<Postgres> = QueryBuilder::new(
            "INSERT INTO chat_room_participant (user_id, room_id, joined_at) "
        );
        builder.push_values(&participants_entities, |mut db, user| {
            db.push_bind(user.user_id)
                .push_bind(user.room_id)
                .push_bind(user.joined_at);
        }).build().fetch_all(&mut *tx).await?;
        tx.commit().await?;
        Ok((room, participants_entities))
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