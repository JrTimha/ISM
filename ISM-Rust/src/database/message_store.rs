use std::sync::Arc;
use scylla::{Session, SessionBuilder};
use scylla::transport::ClusterData;
use scylla::transport::errors::NewSessionError;
use crate::core::ISMConfig;
use tokio::sync::OnceCell;

static DB_INSTANCE: OnceCell<Arc<DatabaseRepository>> = OnceCell::const_new();

pub async fn init_db(config: &ISMConfig) {
    DB_INSTANCE
        .get_or_init(|| async {
        let db = DatabaseRepository::new(config).await.expect("Failed to initialize CassandraDb");
        Arc::new(db)
    }).await;
}

pub async fn get_db_instance() -> Arc<DatabaseRepository> {
    DB_INSTANCE.get().expect("DB instance not initialized. Please call init_db first.").clone()
}

pub struct DatabaseRepository {
    session: Arc<Session>,
}

impl DatabaseRepository {

    async fn new(config: &ISMConfig) -> Result<Self, NewSessionError> {
        let session = SessionBuilder::new()
            .known_node(&config.db_url)
            .user(&config.db_user, &config.db_password)
            .build()
            .await?;
        Ok(DatabaseRepository { session: Arc::new(session) })
    }

    pub async fn fetch_data(&self) -> Result<String, scylla::transport::errors::QueryError> {
        Ok("Data fetched successfully".to_string())
    }

    pub async fn insert_data(&self) -> Result<String, scylla::transport::errors::QueryError> {
        Ok("Data inserted successfully".to_string())
    }

    pub async fn test_connection(&self) -> Result<Arc<ClusterData>, scylla::transport::errors::RequestError> {
        let session = self.session.clone();
        let data = session.get_cluster_data();
        Ok(data)
    }

}




