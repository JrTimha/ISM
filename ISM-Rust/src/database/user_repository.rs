use async_trait::async_trait;
use log::{error, info};
use sqlx::{Pool, Postgres};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;
use crate::core::{UserDbConfig};
use crate::database::user::User;

#[derive(Debug, Clone)]
pub struct UserDbClient {
    pool: Pool<Postgres>,
}

impl UserDbClient {
    pub fn new(pool: Pool<Postgres>) -> Self {
        UserDbClient { pool }
    }
}

#[async_trait]
pub trait UserRepository {

    async fn get_user(
        &self,
        user_id: Uuid
    ) -> Result<Option<User>, sqlx::Error>;

}

#[async_trait]
impl UserRepository for UserDbClient {

    async fn get_user(&self, user_id: Uuid) -> Result<Option<User>, sqlx::Error> {
        let user = sqlx::query_as!(
                User,
                r#"SELECT id, display_name FROM app_user WHERE id = $1"#,
                user_id
            ).fetch_optional(&self.pool).await?;
        Ok(user)
    }
}

pub async fn init_user_db(config: &UserDbConfig) -> UserDbClient {
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
    UserDbClient::new(pool)
}