use std::fs;
use std::path::PathBuf;
use async_trait::async_trait;
use anyhow::Error;
use tracing::log::{debug, info};
use crate::config::{IFSStoreLocalConfig, IFSStoreS3Config};
use crate::stream::ByteStream;
use aws_config::BehaviorVersion;
#[async_trait]
pub trait IFSObjectStore {

    async fn get(&self, object_key: &str)-> Result<Vec<u8>, Error>;

    async fn get_stream(&self, object_key: &str) -> ByteStream;

    async fn put(&self, object_key: &str, data: Vec<u8>) -> Result<(), Error>;

    async fn delete(&self, object_key: &str) -> Result<(), Error>;

}

pub struct AwsS3IFSObjectStore{
    client: aws_sdk_s3::Client,
    bucket_name: String,
    object_prefix: String,
}

impl AwsS3IFSObjectStore{
    pub async fn new(config: &IFSStoreS3Config) -> Self {
        info!(
            "S3 Component Object Store bucket: {}, prefix: {}",
            config.bucket_name, config.object_prefix
        );
        let sdk_config = aws_config::load_defaults(BehaviorVersion::v2024_03_28()).await;
        let client = aws_sdk_s3::Client::new(&sdk_config);
        Self {
            client,
            bucket_name: config.bucket_name.clone(),
            object_prefix: config.object_prefix.clone(),
        }
    }

}

#[async_trait]
impl IFSObjectStore for AwsS3IFSObjectStore {
    async fn get(&self, object_key: &str) -> Result<Vec<u8>, Error> {
        todo!()
    }

    async fn get_stream(&self, object_key: &str) -> ByteStream {
        todo!()
    }

    async fn put(&self, object_key: &str, data: Vec<u8>) -> Result<(), Error> {
        todo!()
    }

    async fn delete(&self, object_key: &str) -> Result<(), Error> {
        todo!()
    }
}

pub struct FsIFSObjectStore{
    root_path: String,
    object_prefix: String,
}

impl FsIFSObjectStore {
    pub fn new(config: &IFSStoreLocalConfig) -> Result<Self, String> {
        let root_dir = std::path::PathBuf::from(config.root_path.as_str());

        if !root_dir.exists() {
            fs::create_dir_all(&root_dir.clone()).map_err(|err| err.to_string())?;
        }

        // Log the initialized values of root_dir and config.root_path
        info!(
        "Initialized Fs IFS Object Store with root_dir: {} and object_prefix: {}",
        root_dir.display(),
        config.root_path.as_str()
    );

        // Returning root_path and object_prefix for further debugging
        Ok(Self {
            root_path: config.root_path.clone(),
            object_prefix: config.root_path.clone(),
        })
    }

    fn get_dir_path(&self) -> PathBuf {
        let root_path = std::path::PathBuf::from(self.root_path.as_str());
        if self.object_prefix.is_empty() {
            root_path
        } else {
            root_path.join(self.object_prefix.as_str())
        }
    }
}

#[async_trait]
impl IFSObjectStore for FsIFSObjectStore {
    async fn get(&self, object_key: &str) -> Result<Vec<u8>, anyhow::Error> {
        let dir_path = self.get_dir_path();

        tracing::debug!("Getting object: {}/{}", dir_path.display(), object_key);

        let file_path = dir_path.join(object_key);

        if file_path.exists() {
            fs::read(file_path).map_err(|e| e.into())
        } else {
            Err(anyhow::Error::msg("Object not found"))
        }
    }

    async fn get_stream(&self, object_key: &str) -> ByteStream {
        let dir_path  = self.get_dir_path();


        info!("Getting object: {}/{}", dir_path.display(), object_key);
        info!("object {}", object_key);
        info!("dir_path {}", dir_path.display());


        let file_path = dir_path.join(object_key);
        info!("Getting file: {}", file_path.display());




        match aws_sdk_s3::primitives::ByteStream::from_path(file_path).await {
            Ok(stream) => stream.into(),
            Err(error) => ByteStream::error(error),
        }
    }

    async fn put(&self, object_key: &str, data: Vec<u8>) -> Result<(), Error> {
        let dir_path  = self.get_dir_path();
        info!("Putting object: {}/{}", dir_path.display(), object_key);
        debug!("Putting object: {}/{}", dir_path.display(), object_key);
        info!("Directory path : {}", dir_path.display());
        info!("object key: {}", object_key);
        if !dir_path.exists() {
            fs::create_dir_all(dir_path.clone())?;
        }

        let file_path = dir_path.join(object_key);
        info!("Full file path: {:?}", file_path.display());
        // let file_path = "/component_store/ifs/c814/read-only//file1.txt";

        if let Some(parent_dir) = file_path.parent() {
            if !parent_dir.exists() {
                info!("Creating directory: {}", parent_dir.display());
                fs::create_dir_all(parent_dir.clone())?;
            }
        }

        fs::write(file_path, data).map_err(|e| e.into())

    }

    async fn delete(&self, object_key: &str) -> Result<(), Error> {
        let dir_path = self.get_dir_path();

        tracing::debug!("Deleting object: {}/{}", dir_path.display(), object_key);

        if !dir_path.exists() {
            fs::create_dir_all(dir_path.clone())?;
        }

        let file_path = dir_path.join(object_key);

        if file_path.exists() {
            fs::remove_file(file_path)?;
        }

        Ok(())
    }
}