// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::empty_worker_metadata;
use crate::gateway_binding::WorkerDetail;
use crate::getter::{get_response_headers_or_default, get_status_code};
use crate::service::component::{ComponentService, ComponentServiceError};
use crate::service::worker::{WorkerService, WorkerServiceError};
use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use futures_util::TryStreamExt;
use golem_common::model::{ComponentFilePath, HasAccountId, TargetWorkerId};
use golem_service_base::auth::EmptyAuthCtx;
use golem_service_base::model::validate_worker_name;
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::{Value, ValueAndType};
use http::StatusCode;
use poem::web::headers::ContentType;
use rib::RibResult;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;

#[async_trait]
pub trait FileServerBindingHandler<Namespace> {
    async fn handle_file_server_binding_result(
        &self,
        namespace: &Namespace,
        worker_detail: &WorkerDetail,
        original_result: RibResult,
    ) -> FileServerBindingResult;
}

pub type FileServerBindingResult = Result<FileServerBindingSuccess, FileServerBindingError>;

pub struct FileServerBindingSuccess {
    pub binding_details: FileServerBindingDetails,
    pub data: Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static>>,
}

pub enum FileServerBindingError {
    InternalError(String),
    InvalidRibResult(String),
    WorkerServiceError(WorkerServiceError),
    ComponentServiceError(ComponentServiceError),
}

#[derive(Debug, Clone)]
pub struct FileServerBindingDetails {
    pub content_type: ContentType,
    pub status_code: StatusCode,
    pub file_path: ComponentFilePath,
}

pub struct DefaultFileServerBindingHandler {
    component_service: Arc<dyn ComponentService<EmptyAuthCtx> + Sync + Send>,
    initial_component_files_service: Arc<InitialComponentFilesService>,
    worker_service: Arc<dyn WorkerService + Sync + Send>,
}

impl DefaultFileServerBindingHandler {
    pub fn new(
        component_service: Arc<dyn ComponentService<EmptyAuthCtx> + Sync + Send>,
        initial_component_files_service: Arc<InitialComponentFilesService>,
        worker_service: Arc<dyn WorkerService + Sync + Send>,
    ) -> Self {
        DefaultFileServerBindingHandler {
            component_service,
            initial_component_files_service,
            worker_service,
        }
    }
}

#[async_trait]
impl<Namespace: HasAccountId + Send + Sync + 'static> FileServerBindingHandler<Namespace>
    for DefaultFileServerBindingHandler
{
    async fn handle_file_server_binding_result(
        &self,
        namespace: &Namespace,
        worker_detail: &WorkerDetail,
        original_result: RibResult,
    ) -> FileServerBindingResult {
        let binding_details = FileServerBindingDetails::from_rib_result(original_result)
            .map_err(FileServerBindingError::InvalidRibResult)?;

        let component_metadata = self
            .component_service
            .get_by_version(
                &worker_detail.component_id.component_id,
                worker_detail.component_id.version,
                &EmptyAuthCtx(),
            )
            .await
            .map_err(FileServerBindingError::ComponentServiceError)?;

        // if we are serving a read_only file, we can just go straight to the blob storage.
        let matching_file = component_metadata
            .files
            .iter()
            .find(|file| file.path == binding_details.file_path && file.is_read_only());

        if let Some(file) = matching_file {
            let data = self
                .initial_component_files_service
                .get(&namespace.account_id(), &file.key)
                .await
                .map_err(|e| {
                    FileServerBindingError::InternalError(format!(
                        "Failed looking up file in storage: {e}"
                    ))
                })?
                .ok_or(FileServerBindingError::InternalError(format!(
                    "File not found in file storage: {}",
                    file.key
                )))
                .map(|stream| {
                    let mapped =
                        stream.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
                    Box::pin(mapped)
                })?;

            Ok(FileServerBindingSuccess {
                binding_details,
                data,
            })
        } else {
            // Read write files need to be fetched from a running worker.
            // Ask the worker service to get the file contents. If no worker is running, one will be started.
            let worker_name_opt_validated = worker_detail
                .worker_name
                .as_ref()
                .map(|w| validate_worker_name(w).map(|_| w.clone()))
                .transpose()
                .map_err(|e| {
                    FileServerBindingError::InternalError(format!("Invalid worker name: {}", e))
                })?;

            let component_id = worker_detail.component_id.component_id.clone();

            let worker_id = TargetWorkerId {
                component_id,
                worker_name: worker_name_opt_validated.clone(),
            };

            let stream = self
                .worker_service
                .get_file_contents(
                    &worker_id,
                    binding_details.file_path.clone(),
                    empty_worker_metadata(),
                )
                .await
                .map_err(FileServerBindingError::WorkerServiceError)?;

            let stream =
                stream.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()));

            Ok(FileServerBindingSuccess {
                binding_details,
                data: Box::pin(stream),
            })
        }
    }
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
                    .map_err(|e| format!("Invalid mime type: {}", e))?
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
