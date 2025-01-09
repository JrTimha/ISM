use async_trait::async_trait;
use log::error;
use sqlx::{Error, Pool, Postgres};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;
use crate::core::ISMConfig;
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

    async fn get_user(&self, user_id: Uuid) -> Result<Option<User>, Error> {
        todo!()
    }
}

pub async fn init_user_db(_config: &ISMConfig) -> UserDbClient {
    //todo: use config
    let opt = PgConnectOptions::new()
        .host("localhost")
        .port(32768)
        .database("postgres")
        .username("postgres")
        .password("meventure1234");
    let pool = match PgPoolOptions::new()
        .max_connections(10)
        .connect_with(opt)
        .await
    {
        Ok(pool) => {
            println!("✅Connection to the database is successful!");
            pool
        }
        Err(err) => {
            error!("🔥 Failed to connect to the database: {:?}", err);
            std::process::exit(1);
        }
    };
    UserDbClient::new(pool)
}