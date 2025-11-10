use std::sync::Arc;
use log::info;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use tokio::task;
use crate::broadcast::BroadcastChannel;
use crate::cache::redis_cache::{Cache, NoOpCache, RedisCache};
use crate::core::ISMConfig;
use crate::database::{MessageDatabase, ObjectStorage};
use crate::kafka::start_consumer;
use crate::repository::room_repository::RoomRepository;
use crate::repository::user_repository::UserRepository;


#[derive(Clone)]
pub struct AppState {
    pub env: ISMConfig,
    pub room_repository: RoomRepository,
    pub user_repository: UserRepository,
    pub message_repository: MessageDatabase,
    pub cache: Arc<dyn Cache>,
    pub s3_bucket: ObjectStorage
}

impl AppState {
    pub async fn new(config: ISMConfig) -> Self {

        //1: setting up the postgre sql connection for all repositories:
        let options = PgConnectOptions::new()
            .host(&config.user_db_config.db_host)
            .port(config.user_db_config.db_port)
            .database(&config.user_db_config.db_name)
            .username(&config.user_db_config.db_user)
            .password(&config.user_db_config.db_password);
        let pool = match PgPoolOptions::new()
            .max_connections(8)
            .connect_with(options)
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

        let cache: Arc<dyn Cache> = match config.redis_cache_url.clone() {
            Some(url) => {
                let cache = RedisCache::new(url).await
                    .unwrap_or_else(|err| panic!("Unable to init redis cache: {}", err));
                Arc::new(cache)
            },
            None => {
                info!("Redis is deactivated. Initializing NoOpCache...");
                Arc::new(NoOpCache)
            }
        };

        //init broadcaster channel
        BroadcastChannel::init(cache.clone()).await;
        
        //2. State struct:
        let state = Self {
            env: config.clone(),
            room_repository: RoomRepository::new(pool.clone()),
            user_repository: UserRepository::new(pool.clone()),
            message_repository: MessageDatabase::new(&config.message_db_config).await,
            s3_bucket: ObjectStorage::new(&config.object_db_config).await,
            cache: cache
        };

        //3: kafka (optional)
        if state.env.use_kafka == true {
            let kafka_config = state.env.kafka_config.clone();
            task::spawn(async move {
                start_consumer(kafka_config).await;
            });
        }

        state
    }
}