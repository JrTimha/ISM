use crate::core::ObjectStorageConfig;
use bytes::Bytes;
use minio::s3::builders::ObjectContent;
use minio::s3::creds::StaticProvider;
use minio::s3::http::BaseUrl;
use minio::s3::response_traits::{HasObject, HasVersion};
use minio::s3::segmented_bytes::SegmentedBytes;
use minio::s3::types::S3Api;
use minio::s3::{MinioClient, MinioClientBuilder};
use std::sync::Arc;
use tracing::{debug, info};

#[derive(Debug, Clone)]
pub struct ObjectStorage {
    session: Arc<MinioClient>,
    config: ObjectStorageConfig,
}

impl ObjectStorage {
    pub async fn new(config: &ObjectStorageConfig) -> Self {
        let static_provider = StaticProvider::new(&config.access_key, &config.secret_key, None);

        let url = match config.storage_url.parse::<BaseUrl>() {
            Ok(url) => url,
            Err(error) => panic!("Unable to parse s3 url: {:?}", error),
        };

        let client: MinioClient = match MinioClientBuilder::new(url)
            .provider(Some(static_provider))
            .build()
        {
            Ok(client) => client,
            Err(error) => panic!("Unable to initialize client: {:?}", error),
        };
        // Since 0.4 the request builders validate bucket and object names up front and return a
        // `ValidationErr` before anything is sent, so every call site has a second failure mode.
        let bucket_exists = client.bucket_exists(&config.bucket_name)
            .expect("Invalid to build bucket exists request") //
            .build()
            .send()
            .await;
        
        match bucket_exists {
            Ok(response) => {
                info!("Established connection to the s3 storage.");
                if !response.exists() {
                    panic!("The configured bucket does not exist: {:?}", &config.bucket_name);
                }
            }
            Err(error) => {
                panic!("Unable to check if bucket exists: {:?}", error)
            }
        };
        ObjectStorage {
            session: Arc::new(client),
            config: config.clone(),
        }
    }

    pub async fn get_object(
        &self,
        object_id: &String,
    ) -> Result<SegmentedBytes, Box<dyn std::error::Error + Send + Sync>> {
        let session = self.session.clone();
        let response = session
            .get_object(&self.config.bucket_name, object_id)?
            .build()
            .send()
            .await?;
        let object = response.content()?.to_segmented_bytes().await?;
        Ok(object)
    }

    pub async fn delete_object(
        &self,
        object_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let session = self.session.clone();
        let response = session
            .delete_object(&self.config.bucket_name, object_id)?
            .build()
            .send()
            .await?;
        debug!(version_id = ?response.version_id(), "Deleted object");
        Ok(())
    }

    pub async fn insert_object(
        &self,
        object_id: &String,
        content: Bytes,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let session = self.session.clone();
        let object = ObjectContent::from(content);
        let response = session
            .put_object_content(&self.config.bucket_name, object_id, object)?
            .content_type("image/jpeg".to_string())
            .build()
            .send()
            .await?;
        debug!(object = ?response.object(), "Saved object");
        Ok(())
    }
}
