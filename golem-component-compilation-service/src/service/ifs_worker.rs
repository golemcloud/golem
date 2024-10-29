use std::{fs, io};
use std::future::Future;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tracing::{info, warn};
use golem_common::model::ComponentId;
use golem_worker_executor_base::services::ifs::InitialFileSystemService;
use crate::model::{InitialFileSystemToUpload, InitialFileSystemError, ComponentWithVersion, Application, FileProperty, FileSource, Permissions};
use zip::ZipArchive;

#[derive(Clone)]
pub struct InitialFileSystemWorker {
    initial_file_system_service: Arc<dyn InitialFileSystemService + Send + Sync>,
}

impl InitialFileSystemWorker {
    pub fn start(
        initial_file_system_service: Arc<dyn InitialFileSystemService + Send + Sync>,
        mut recv: mpsc::Receiver<InitialFileSystemToUpload>
    ){
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

        let path = Path::new(&component_and_version.id.to_string())
            .join(format!("{}.ifs", component_and_version.version));
        let upload_result = self.initial_file_system_service.put(
            &component_and_version.id,
            component_and_version.version,
            &initial_file_system,
            path.as_path()
         )
            .await
            .map_err(|err| InitialFileSystemError::Unexpected(err.to_string()));

        if let Err(ref err) = upload_result {
            warn!("Failed to upload IFS for component {component_and_version}: {err:?}");
        } else {
            info!("Successfully uploaded IFS for component {component_and_version}");

            if let Err(err) = self.decompress_ifs_zip(&component_and_version).await {
                warn!("Decompression failed for component {}: {:?}", component_and_version.id, err);
            }
        }
    }
    //
    // async fn decompress_ifs_zip(&self, component_and_version: &ComponentWithVersion) -> Result<(), InitialFileSystemError> {
    //     info!("Starting decompression of IFS for component: {}", component_and_version.id);
    //
    //     // Define the path to the compressed file
    //     let zip_path = format!(
    //         "/worker_executor_store/initial_file_system/{}/{}/{}.ifs",
    //         component_and_version.id,
    //         component_and_version.id,
    //         component_and_version.version
    //     );
    //     info!("Zip file path: {}", zip_path);
    //
    //     // Open the zip file
    //     let file = File::open(&zip_path).await.map_err(|e| {
    //         warn!("Failed to open IFS zip file at {}: {}", zip_path, e);
    //         InitialFileSystemError::Unexpected { 0: e.to_string() }
    //     })?;
    //
    //     let mut buffer = Vec::new();
    //     let mut reader = BufReader::new(file);
    //     reader.read_to_end(&mut buffer).await.map_err(|e| {
    //         warn!("Failed to read IFS zip file into memory: {}", e);
    //         InitialFileSystemError::Unexpected { 0: e.to_string() }
    //     })?;
    //
    //     // Create a cursor for in-memory zip data
    //     let cursor = Cursor::new(buffer);
    //
    //     // Initialize ZipArchive
    //     let mut zip = ZipArchive::new(cursor).map_err(|e| {
    //         warn!("Failed to open ZipArchive from IFS buffer: {}", e);
    //         InitialFileSystemError::Unexpected { 0: e.to_string() }
    //     })?;
    //
    //     let base_output_path = PathBuf::from(format!(
    //         "/worker_executor_store/initial_file_system/{}/{}/extracted",
    //         component_and_version.id,
    //         component_and_version.id
    //     ));
    //     info!("Base output path for extraction: {}", base_output_path.display());
    //
    //     let mut extracted_files = Vec::new();
    //
    //     // Iterate through files in the ZIP archive
    //     for i in 0..zip.len() {
    //         let mut file = zip.by_index(i).map_err(|e| {
    //             warn!("Failed to retrieve file at index {} in ZipArchive: {}", i, e);
    //             InitialFileSystemError::Unexpected { 0: e.to_string() }
    //         })?;
    //
    //         let file_name = file.name().to_string();
    //         let out_path = base_output_path.join(&file_name);
    //
    //         let mut file_content = Vec::new();
    //         if std::io::copy(&mut file, &mut file_content).is_err() {
    //             warn!("Failed to read content for file {} in zip", file_name);
    //             continue;
    //         }
    //
    //         extracted_files.push((file_name, file_content));
    //         info!("Extracted file: {}", out_path.display());
    //     }
    //
    //     // Upload extracted files
    //     for (file_name, extracted_content) in extracted_files {
    //         info!("Uploading extracted file: {}", file_name);
    //
    //         let path = PathBuf::from(format!(
    //             "/worker_executor_store/initial_file_system/{}/{}/extracted/{}",
    //             component_and_version.id,
    //             component_and_version.id,
    //             file_name
    //         ));
    //
    //         let upload_result = self.initial_file_system_service.put(
    //             &component_and_version.id,
    //             component_and_version.version,
    //             &extracted_content,
    //             path.as_path()
    //         ).await.map_err(|err| InitialFileSystemError::Unexpected(err.to_string()));
    //
    //         if let Err(ref err) = upload_result {
    //             warn!("Failed to save extracted file {} for component {}: {:?}", file_name, component_and_version.id, err);
    //         } else {
    //             info!("Successfully uploaded file {} for component {}", file_name, component_and_version.id);
    //         }
    //     }
    //
    //     let golem_yaml_path = base_output_path.join("config/golem.yaml");
    //
    //     // /worker_executor_store/initial_file_system/99576e3c-6ec0-4460-b1bf-22a762910c1e/99576e3c-6ec0-4460-b1bf-22a762910c1e/extracted/config
    //     // /worker_executor_store/initial_file_system/99576e3c-6ec0-4460-b1bf-22a762910c1e/0/extracted/config/golem.yaml
    //     info!("------------>>>> {}", golem_yaml_path.display());
    //
    //     let golem_yaml_content = tokio::fs::read_to_string(&golem_yaml_path).await.map_err(|e| {
    //         warn!("Failed to read golem.yaml: {}", e);
    //         InitialFileSystemError::Unexpected { 0: e.to_string() }
    //     })?;
    //
    //     let application : Application = serde_yaml::from_str(golem_yaml_content.as_str()).map_err(|e|{
    //         warn!("Failed to parse golem.yaml: {}", e);
    //         InitialFileSystemError::Unexpected { 0: e.to_string() }
    //     })?;
    //
    //     for component in application.spec.components {
    //         if let Some(files_values) = component.properties.get("files") {
    //             let files: Vec<FileProperty> = serde_json::from_value(files_values.clone()).map_err(|e| {
    //                 warn!("Failed to parse files in golem.yaml: {}", e);
    //                 InitialFileSystemError::Unexpected { 0: e.to_string() }
    //             })?;
    //
    //             for file in files {
    //                 if let FileSource::Url(url) = file.source_path {
    //                     let target_folder = match file.permissions {
    //                         Permissions::ReadOnly => "read-only",
    //                         Permissions::ReadWrite => "read-write",
    //                     };
    //                     let target_path = base_output_path.join(target_folder).join(&file.target_path);
    //
    //                     // Download the file content
    //                     let response = reqwest::get(&url).await.map_err(|e| {
    //                         warn!("Failed to download file from URL {}: {}", url, e);
    //                         InitialFileSystemError::Unexpected { 0: e.to_string() }
    //                     })?;
    //                     let content = response.bytes().await.map_err(|e| {
    //                         warn!("Failed to read downloaded content from {}: {}", url, e);
    //                         InitialFileSystemError::Unexpected { 0: e.to_string() }
    //                     })?;
    //
    //                     // Write the file to the target path
    //                     tokio::fs::create_dir_all(target_path.parent().unwrap()).await.expect("TODO: panic message");
    //                     let mut file = tokio::fs::File::create(target_path).await.expect("");
    //                     file.write_all(&content).await?;
    //                 }
    //             }
    //         }
    //     }
    //
    //
    //
    //     info!("Decompression and upload complete for component: {}", component_and_version.id);
    //     Ok(())
    // }

    async fn decompress_ifs_zip(&self, component_and_version: &ComponentWithVersion) -> Result<(), InitialFileSystemError> {
        info!("Starting decompression of IFS for component: {}", component_and_version.id);

        // Define the path to the compressed file
        let zip_path = format!(
            "/worker_executor_store/initial_file_system/{}/{}/{}.ifs",
            component_and_version.id,
            component_and_version.id,
            component_and_version.version
        );
        info!("Zip file path: {}", zip_path);

        // Open the zip file
        let file = File::open(&zip_path).await.map_err(|e| {
            warn!("Failed to open IFS zip file at {}: {}", zip_path, e);
            InitialFileSystemError::Unexpected { 0: e.to_string() }
        })?;

        let mut buffer = Vec::new();
        let mut reader = BufReader::new(file);
        reader.read_to_end(&mut buffer).await.map_err(|e| {
            warn!("Failed to read IFS zip file into memory: {}", e);
            InitialFileSystemError::Unexpected { 0: e.to_string() }
        })?;

        // Create a cursor for in-memory zip data
        let cursor = Cursor::new(buffer);

        // Initialize ZipArchive
        let mut zip = ZipArchive::new(cursor).map_err(|e| {
            warn!("Failed to open ZipArchive from IFS buffer: {}", e);
            InitialFileSystemError::Unexpected { 0: e.to_string() }
        })?;

        let base_output_path = PathBuf::from(format!(
            "/worker_executor_store/initial_file_system/{}/{}/extracted",
            component_and_version.id,
            component_and_version.id
        ));
        info!("Base output path for extraction: {}", base_output_path.display());

        let mut extracted_files = Vec::new();

        // Iterate through files in the ZIP archive
        for i in 0..zip.len() {
            let mut file = zip.by_index(i).map_err(|e| {
                warn!("Failed to retrieve file at index {} in ZipArchive: {}", i, e);
                InitialFileSystemError::Unexpected { 0: e.to_string() }
            })?;

            let file_name = file.name().to_string();
            let out_path = base_output_path.join(&file_name);

            let mut file_content = Vec::new();
            if std::io::copy(&mut file, &mut file_content).is_err() {
                warn!("Failed to read content for file {} in zip", file_name);
                continue;
            }

            extracted_files.push((file_name.clone(), file_content));
            info!("Extracted file: {}", out_path.display());
        }

        // Upload extracted files
        for (file_name, extracted_content) in &extracted_files {
            let path = base_output_path.join(file_name);
            let upload_result = self.initial_file_system_service
                .put(
                    &component_and_version.id,
                    component_and_version.version,
                    extracted_content,
                    path.as_path(),
                )
                .await
                .map_err(|err| InitialFileSystemError::Unexpected(err.to_string()));

            if let Err(ref err) = upload_result {
                warn!("Failed to save extracted file {} for component {}: {:?}", file_name, component_and_version.id, err);
            } else {
                info!("Successfully uploaded file {} for component {}", file_name, component_and_version.id);
            }
        }

        // Read golem.yaml, parse, and process URLs
        let golem_yaml_path = base_output_path.join("config/golem.yaml");
        let golem_yaml_content = tokio::fs::read_to_string(&golem_yaml_path).await.map_err(|e| {
            warn!("Failed to read golem.yaml: {}", e);
            InitialFileSystemError::Unexpected { 0: e.to_string() }
        })?;

        let application: Application = serde_yaml::from_str(golem_yaml_content.as_str()).map_err(|e| {
            warn!("Failed to parse golem.yaml: {}", e);
            InitialFileSystemError::Unexpected { 0: e.to_string() }
        })?;

        // Download files based on URL and save via `put`
        for component in application.spec.components {
            if let Some(files_values) = component.properties.get("files") {

                info!("-=------------------------------------ {}", files_values);

                let files: Vec<FileProperty> = serde_json::from_value(files_values.clone()).map_err(|e| {
                    warn!("Failed to parse files in golem.yaml: {}", e);
                    InitialFileSystemError::Unexpected { 0: e.to_string() }
                })?;

                for file in files {
                    if let FileSource::Url(url) = file.source_path {
                        let target_folder = match file.permissions {
                            Permissions::ReadOnly => "read-only",
                            Permissions::ReadWrite => "read-write",
                        };
                        let target_path = base_output_path
                            .join(target_folder)
                            .join(&file.target_path.strip_prefix('/').unwrap_or(&file.target_path));


                        // Download the file content
                        let response = reqwest::get(&url).await.map_err(|e| {
                            warn!("Failed to download file from URL {}: {}", url, e);
                            InitialFileSystemError::Unexpected { 0: e.to_string() }
                        })?;
                        let content = response.bytes().await.map_err(|e| {
                            warn!("Failed to read downloaded content from {}: {}", url, e);
                            InitialFileSystemError::Unexpected { 0: e.to_string() }
                        })?;

                        // Use `put` to save downloaded content
                        self.initial_file_system_service.put(
                            &component_and_version.id,
                            component_and_version.version,
                            &content.to_vec(),
                            &target_path,
                        )
                            .await
                            .map_err(|err| {
                                warn!("Failed to upload downloaded file {}: {}", target_path.display(), err);
                                InitialFileSystemError::Unexpected(err.to_string())
                            })?;
                    }
                }
            }
        }
        
        info!("Setting up the folder's permission");
        self.initial_file_system_service.set_permissions(base_output_path.as_path()).await.expect("Setting up permission failed");

        info!("Decompression and upload complete for component: {}", component_and_version.id);
        Ok(())
    }

}
