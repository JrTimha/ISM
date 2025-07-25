use std::sync::Arc;
use bytes::Bytes;
use log::{debug, info};
use minio::s3::{Client, ClientBuilder};
use minio::s3::builders::{ObjectContent, ObjectToDelete};
use minio::s3::creds::StaticProvider;
use minio::s3::http::BaseUrl;
use minio::s3::segmented_bytes::SegmentedBytes;
use minio::s3::types::S3Api;
use crate::core::ObjectDbConfig;

#[derive(Debug, Clone)]
pub struct ObjectDatabase {
    session: Arc<Client>,
    config: ObjectDbConfig,
}

impl ObjectDatabase {

    pub async fn new(config: &ObjectDbConfig) -> Self {
        let static_provider = Box::new(StaticProvider::new(
            &config.db_user,
            &config.db_password,
            None,
        ));
        let url = match config.db_url.parse::<BaseUrl>() {
            Ok(url) => url,
            Err(error) => panic!("Unable to parse db url: {:?}", error)
        };
        let client: Client = match ClientBuilder::new(url).provider(Some(static_provider)).build() {
            Ok(client) => client,
            Err(error) => panic!("Unable to initialize client: {:?}", error)
        };
        match client.bucket_exists(&config.bucket_name).send().await {
            Ok(buckets) => {
                info!("Established connection to the s3 storage.");
                if buckets.exists == false {
                    panic!("The configured bucket does not exist: {:?}", &config.bucket_name);
                }
            },
            Err(error) => {
                panic!("Unable to check if bucket exists: {:?}", error)
            }
        };
        ObjectDatabase { session: Arc::new(client), config: config.clone() }
    }

    pub async fn get_object(&self, object_id: &String) -> Result<SegmentedBytes,  Box<dyn std::error::Error + Send + Sync>> {
        let session = self.session.clone();
        let response = session.get_object(&self.config.bucket_name, object_id).send().await?;
        let object = response.content.to_segmented_bytes().await?;
        Ok(object)
    }

    pub async fn delete_object(&self, object_id: &String) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let session = self.session.clone();
        let response = session.delete_object(&self.config.bucket_name, ObjectToDelete::from(object_id)).send().await?;
        debug!("Deleted object, marker: {:?}", response.version_id);
        Ok(())
    }

    pub async fn insert_object(&self, object_id: &String, content: Bytes) -> Result<(), Box<dyn std::error::Error+Send+Sync>> {
        let session = self.session.clone();
        let object = ObjectContent::from(content);
        let response = session.put_object_content(&self.config.bucket_name, object_id, object).content_type("image/jpeg".to_string()).send().await?;
        debug!("Saved object with name: {:?}", response.object);
        Ok(())
    }

}