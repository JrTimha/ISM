use crate::broadcast::BroadcastChannel;
use crate::cache::redis_cache::{Cache, NoOpCache, RedisCache};
use crate::core::ISMConfig;
use crate::kafka::PushNotificationProducer;
use crate::messaging::chat_repository::ChatRepository;
use crate::object_storage::ObjectStorage;
use crate::rooms::room_repository::RoomRepository;
use crate::users::user_repository::UserRepository;
use log::info;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub env: ISMConfig,
    pub room_repository: RoomRepository,
    pub user_repository: UserRepository,
    pub chat_repository: ChatRepository,
    pub cache: Arc<dyn Cache>,
    pub s3_bucket: ObjectStorage,
}

impl AppState {
    pub async fn new(config: ISMConfig) -> Self {
        //1: setting up the postgresql connection for all repositories:
        let options = PgConnectOptions::new()
            .host(&config.room_db_config.db_host)
            .port(config.room_db_config.db_port)
            .database(&config.room_db_config.db_name)
            .username(&config.room_db_config.db_user)
            .password(&config.room_db_config.db_password);

        let pool = match PgPoolOptions::new()
            .max_connections(20)
            .connect_with(options)
            .await
        {
            Ok(pool) => {
                info!("Established connection to the PostgreSQL database.");
                pool
            }
            Err(err) => {
                panic!("Failed to connect to the PostgreSQL database: {:?}", err);
            }
        };

        //2: init redis cache, if present:
        let cache: Arc<dyn Cache> = match config.redis_cache_url.clone() {
            Some(url) => {
                let cache = RedisCache::new(url)
                    .await
                    .unwrap_or_else(|err| panic!("Unable to init redis cache: {}", err));
                Arc::new(cache)
            }
            None => {
                info!("Redis is deactivated. Initializing NoOpCache...");
                Arc::new(NoOpCache)
            }
        };

        //3. init broadcaster channel:
        BroadcastChannel::init(
            cache.clone(),
            PushNotificationProducer::new(config.use_kafka, config.kafka_config.clone()),
        )
        .await;

        //4. init application state:
        let state = Self {
            env: config.clone(),
            room_repository: RoomRepository::new(pool.clone()),
            user_repository: UserRepository::new(pool.clone()),
            chat_repository: ChatRepository::new(pool.clone()),
            s3_bucket: ObjectStorage::new(&config.object_db_config).await,
            cache: cache,
        };

        state
    }
}
