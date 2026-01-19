// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::getter::{get_response_headers_or_default, get_status_code};
use crate::service::component::{ComponentService, ComponentServiceError};
use crate::service::worker::{WorkerService, WorkerServiceError};
use bytes::Bytes;
use futures::Stream;
use futures::TryStreamExt;
use golem_common::model::account::AccountId;
use golem_common::model::component::{ComponentDto, ComponentFilePath, ComponentId};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::WorkerId;
use golem_common::SafeDisplay;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_wasm::analysis::AnalysedType;
use golem_wasm::{Value, ValueAndType};
use http::StatusCode;
use poem::web::headers::ContentType;
use rib::RibResult;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;

pub struct FileServerBindingHandler {
    component_service: Arc<dyn ComponentService>,
    initial_component_files_service: Arc<InitialComponentFilesService>,
    worker_service: Arc<WorkerService>,
}

impl FileServerBindingHandler {
    pub fn new(
        component_service: Arc<dyn ComponentService>,
        initial_component_files_service: Arc<InitialComponentFilesService>,
        worker_service: Arc<WorkerService>,
    ) -> Self {
        Self {
            component_service,
            initial_component_files_service,
            worker_service,
        }
    }

    pub async fn handle_file_server_binding_result(
        &self,
        worker_name: &str,
        component_id: ComponentId,
        environment_id: EnvironmentId,
        account_id: AccountId,
        original_result: RibResult,
    ) -> Result<FileServerBindingSuccess, FileServerBindingError> {
        let binding_details = FileServerBindingDetails::from_rib_result(original_result)
            .map_err(FileServerBindingError::InvalidRibResult)?;

        let component_metadata = self
            .get_component_metadata(worker_name, component_id, account_id)
            .await?;

        // if we are serving a read_only file, we can just go straight to the blob storage.
        let matching_ro_file = component_metadata
            .files
            .iter()
            .find(|file| file.path == binding_details.file_path && file.is_read_only());

        if let Some(file) = matching_ro_file {
            let data = self
                .initial_component_files_service
                .get(environment_id, file.content_hash)
                .await
                .map_err(|e| {
                    FileServerBindingError::InternalError(format!(
                        "Failed looking up file in storage: {e}"
                    ))
                })?
                .ok_or(FileServerBindingError::InternalError(format!(
                    "File not found in file storage: {}",
                    file.content_hash
                )))
                .map(|stream| {
                    let mapped = stream.map_err(std::io::Error::other);
                    Box::pin(mapped)
                })?;

            Ok(FileServerBindingSuccess {
                binding_details,
                data,
            })
        } else {
            // Read write files need to be fetched from a running worker.
            // Ask the worker service to get the file contents. If no worker is running, one will be started.

            let worker_id = WorkerId::from_component_metadata_and_worker_id(
                component_id,
                &component_metadata.metadata,
                worker_name,
            )
            .map_err(|e| {
                FileServerBindingError::InternalError(format!("Invalid worker name: {e}"))
            })?;

            let stream = self
                .worker_service
                .get_file_contents(
                    &worker_id,
                    binding_details.file_path.clone(),
                    AuthCtx::impersonated_user(account_id),
                )
                .await?;

            let stream = stream.map_err(|e| std::io::Error::other(e.to_string()));

            Ok(FileServerBindingSuccess {
                binding_details,
                data: Box::pin(stream),
            })
        }
    }

    async fn get_component_metadata(
        &self,
        worker_name: &str,
        component_id: ComponentId,
        account_id: AccountId,
    ) -> Result<ComponentDto, FileServerBindingError> {
        // Two cases, we either have an existing worker or not (either not configured or not existing).
        // If there is no worker we need use the lastest component version, if there is none we need to use the exact component version
        // the worker is using. Not doing that would make the blob_storage optimization for read-only files visible to users.

        let component_revision = {
            let worker_metadata = self
                .worker_service
                .get_metadata(
                    &WorkerId {
                        component_id,
                        worker_name: worker_name.to_string(),
                    },
                    AuthCtx::impersonated_user(account_id),
                )
                .await;

            match worker_metadata {
                Ok(metadata) => Some(metadata.component_revision),
                Err(WorkerServiceError::WorkerNotFound(_)) => None,
                Err(other) => Err(other)?,
            }
        };

        let component_metadata = if let Some(component_revision) = component_revision {
            self.component_service
                .get_revision(component_id, component_revision)
                .await
                .map_err(FileServerBindingError::ComponentServiceError)?
        } else {
            self.component_service
                .get_latest_by_id(component_id)
                .await
                .map_err(FileServerBindingError::ComponentServiceError)?
        };

        Ok(component_metadata)
    }
}

pub struct FileServerBindingSuccess {
    pub binding_details: FileServerBindingDetails,
    pub data: Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static>>,
}

#[derive(Debug, thiserror::Error)]
pub enum FileServerBindingError {
    #[error(transparent)]
    WorkerServiceError(#[from] WorkerServiceError),
    #[error(transparent)]
    ComponentServiceError(#[from] ComponentServiceError),
    #[error("Internal error: {0}")]
    InternalError(String),
    #[error("Invalid rib result: {0}")]
    InvalidRibResult(String),
}

impl SafeDisplay for FileServerBindingError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::WorkerServiceError(inner) => inner.to_safe_string(),
            Self::ComponentServiceError(inner) => inner.to_safe_string(),

            Self::InternalError(_) => self.to_string(),
            Self::InvalidRibResult(_) => self.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileServerBindingDetails {
    pub content_type: ContentType,
    pub status_code: StatusCode,
    pub file_path: ComponentFilePath,
}

impl FileServerBindingDetails {
    pub fn from_rib_result(result: RibResult) -> Result<FileServerBindingDetails, String> {
        // Three supported formats:
        // 1. A string path. Mime type is guessed from the path. Status code is 200.
        // 2. A record with a 'file-path' field. Mime type and status are optionally taken from the record, otherwise guessed.
        // 3. A result of either of the above, with the same rules applied.
        match result {
            RibResult::Val(value) => match value {
                ValueAndType {
                    value: Value::Result(value),
                    typ: AnalysedType::Result(typ),
                } => match value {
                    Ok(ok) => {
                        let ok = ValueAndType::new(
                            *ok.ok_or("ok unset".to_string())?,
                            (*typ.ok.ok_or("Missing 'ok' type")?).clone(),
                        );
                        Self::from_rib_happy(ok)
                    }
                    Err(err) => {
                        let value = err.ok_or("err unset".to_string())?;
                        Err(format!("Error result: {value:?}"))
                    }
                },
                other => Self::from_rib_happy(other),
            },
            RibResult::Unit => Err("Expected a value".to_string()),
        }
    }

    /// Like the above, just without the result case.
    fn from_rib_happy(value: ValueAndType) -> Result<FileServerBindingDetails, String> {
        match &value {
            ValueAndType {
                value: Value::String(raw_path),
                ..
            } => Self::make_from(raw_path.clone(), None, None),
            ValueAndType {
                value: Value::Record(field_values),
                typ: AnalysedType::Record(record),
            } => {
                let path_position = record
                    .fields
                    .iter()
                    .position(|pair| &pair.name == "file-path")
                    .ok_or("Record must contain 'file-path' field")?;

                let path = if let Value::String(path) = &field_values[path_position] {
                    path
                } else {
                    return Err("file-path must be a string".to_string());
                };

                let status = get_status_code(field_values, record)?;
                let headers = get_response_headers_or_default(&value)?;
                let content_type = headers.get_content_type();

                Self::make_from(path.to_string(), content_type, status)
            }
            _ => Err("Response value expected".to_string()),
        }
    }

    fn make_from(
        path: String,
        content_type: Option<ContentType>,
        status_code: Option<StatusCode>,
    ) -> Result<FileServerBindingDetails, String> {
        let file_path = ComponentFilePath::from_either_str(&path)?;

        let content_type = match content_type {
            Some(content_type) => content_type,
            None => {
                let mime_type = mime_guess::from_path(&path)
                    .first()
                    .ok_or("Could not determine mime type")?;
                ContentType::from_str(mime_type.as_ref())
                    .map_err(|e| format!("Invalid mime type: {e}"))?
            }
        };

        let status_code = status_code.unwrap_or(StatusCode::OK);

        Ok(FileServerBindingDetails {
            status_code,
            content_type,
            file_path,
        })
    }
}
