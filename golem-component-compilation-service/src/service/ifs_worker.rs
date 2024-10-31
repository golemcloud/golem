use std::{fs, io};
use std::future::Future;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tracing::{info, warn};
use golem_common::model::{ComponentId, OwnedWorkerId};
// use golem_worker_executor_base::services::ifs::InitialFileSystemService;
use crate::model::{InitialFileSystemToUpload, InitialFileSystemError, ComponentWithVersion, Application, FileProperty, FileSource, Permissions};
use zip::ZipArchive;
use golem_worker_executor_base::services::blob_store::BlobStoreService;

#[derive(Clone)]
pub struct InitialFileSystemWorker {
    initial_file_system_service: Arc<dyn BlobStoreService + Send + Sync>,
}

impl InitialFileSystemWorker {
    pub fn start(
        initial_file_system_service: Arc<dyn BlobStoreService + Send + Sync>,
        mut recv: mpsc::Receiver<InitialFileSystemToUpload>
    ) {
        let worker = Self {
            initial_file_system_service
        };

        tokio::spawn(async move {
            loop {
                while let Some(request) = recv.recv().await {
                    worker.upload_initial_file_system(request).await;
                }
            }
        });
    }

    async fn upload_initial_file_system(&self, initial_file: InitialFileSystemToUpload) {
        info!("Uploading initial file");


        let InitialFileSystemToUpload {
            initial_file_system,
            component_and_version
        } = initial_file;

        self.initial_file_system_service.
            save_ifs_zip(
                initial_file_system,
                component_and_version.clone().id,
                component_and_version.version
            ).await.expect("Failed to save ifs");

    }
}