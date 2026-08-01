use crate::core::{ObjectStorageConfig, StartupError, StartupResult};
use bytes::Bytes;
use minio::s3::builders::ObjectContent;
use minio::s3::creds::StaticProvider;
use minio::s3::error::Error;
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
    /// Connects to the object storage and verifies the configured bucket exists.
    ///
    /// Every failure here — a malformed URL, an unreachable host, a missing bucket — is a
    /// misconfiguration or a dependency that has not come up yet. Those are ordinary startup
    /// outcomes, so they are returned as [`StartupError`] rather than unwound as panics.
    pub async fn connect(config: &ObjectStorageConfig) -> StartupResult<Self> {
        let static_provider = StaticProvider::new(&config.access_key, &config.secret_key, None);

        let url = config
            .storage_url
            .parse::<BaseUrl>()
            .map_err(|error| StartupError::ObjectStorage(format!("invalid S3 url: {error}")))?;

        let client: MinioClient = MinioClientBuilder::new(url)
            .provider(Some(static_provider))
            .build()
            .map_err(|error| StartupError::ObjectStorage(format!("could not build the client: {error}")))?;

        // Since 0.4 the request builders validate bucket and object names up front and return a
        // `ValidationErr` before anything is sent, so every call site has a second failure mode.
        let response = client
            .bucket_exists(&config.bucket_name)
            .map_err(|error| StartupError::ObjectStorage(format!("invalid bucket name: {error}")))?
            .build()
            .send()
            .await
            .map_err(|error| StartupError::ObjectStorage(format!("could not check if the bucket exists: {error}")))?;

        if !response.exists() {
            return Err(StartupError::ObjectStorage(format!(
                "the configured bucket does not exist: {}",
                config.bucket_name
            )));
        }

        info!("Established connection to the S3 Object Storage.");
        Ok(ObjectStorage {
            session: Arc::new(client),
            config: config.clone(),
        })
    }

    pub async fn get_object(&self, object_id: &String) -> Result<SegmentedBytes, Box<dyn std::error::Error + Send + Sync>> {
        let session = self.session.clone();
        let response = session.get_object(&self.config.bucket_name, object_id)?.build().send().await?;
        let object = response.content()?.to_segmented_bytes().await?;
        Ok(object)
    }

    pub async fn delete_object(&self, object_id: &str) -> Result<(), Error> {
        let session = self.session.clone();
        let response = session.delete_object(&self.config.bucket_name, object_id)?.build().send().await?;
        debug!(version_id = ?response.version_id(), "Deleted object");
        Ok(())
    }

    pub async fn insert_object(&self, object_id: &String, content: Bytes) -> Result<(), Error> {
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
