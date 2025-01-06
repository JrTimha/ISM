use scylla::SessionBuilder;
use scylla::transport::errors::NewSessionError;
use crate::core::ISMConfig;

#[async_trait::async_trait]
pub trait Database {
    async fn insert(&self, collection: &str, data: &str) -> Result<(), String>;
    async fn find(&self, collection: &str, query: &str) -> Result<String, String>;
}

pub struct CassandraDb {
    pub session: scylla::Session,
}

impl CassandraDb {
    pub async fn new(config: &ISMConfig) -> Result<Self, NewSessionError> {
        let session = SessionBuilder::new()
            .known_node(&config.db_url)
            .user(&config.db_user, &config.db_password)
            .build()
            .await?;
        Ok(CassandraDb { session })
    }
}

#[async_trait::async_trait]
impl Database for CassandraDb {
    async fn insert(&self, collection: &str, data: &str) -> Result<(), String> {
        // Cassandra spezifische Logik
        Ok(())
    }

    async fn find(&self, collection: &str, query: &str) -> Result<String, String> {
        // Cassandra spezifische Logik
        Ok("Cassandra data".to_string())
    }
}