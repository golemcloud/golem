// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use super::{
    CanonicalFilePath, FileByteSelection, FileReadError, FileReadResponse, GetFileContentsRequest,
    GetFileSystemNodeRequest, GetFileSystemNodeResponse, GetFileSystemNodeResult, HasAll,
    UsesAllDeps, WorkerCtx, WorkerExecutorError, WorkerExecutorImpl, golem,
    validate_file_read_path,
};
use golem_common::model::filesystem::FileReadHead;

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static>
    WorkerExecutorImpl<Ctx, Svcs>
{
    pub(super) async fn get_file_system_node_internal(
        &self,
        request: GetFileSystemNodeRequest,
    ) -> Result<GetFileSystemNodeResponse, WorkerExecutorError> {
        Self::validate_auth_ctx(&request.auth_ctx)?;

        let path = CanonicalFilePath::from_abs_str(&request.path)
            .map_err(|e| WorkerExecutorError::invalid_request(format!("Invalid path: {e}")))?;

        let result = match self.get_existing_running(&request).await {
            Ok(worker) => {
                Self::not_found_if_deleting(&worker, worker.get_file_system_node(path).await).await
            }
            Err(error) => Err(error),
        };
        let result = match result {
            Ok(result) => result,
            Err(WorkerExecutorError::AgentNotFound { .. }) => GetFileSystemNodeResult::NotFound,
            Err(error) => return Err(error),
        };

        let response = match result {
            GetFileSystemNodeResult::Ok(entries) => GetFileSystemNodeResponse {
                result: Some(
                    golem::workerexecutor::v1::get_file_system_node_response::Result::DirSuccess(
                        golem::workerexecutor::v1::ListDirectorySuccessResponse {
                            nodes: entries.into_iter().map(|entry| entry.into()).collect(),
                        },
                    ),
                ),
            },
            GetFileSystemNodeResult::NotFound => GetFileSystemNodeResponse {
                result: Some(
                    golem::workerexecutor::v1::get_file_system_node_response::Result::NotFound(
                        golem_schema::proto::golem::common::Empty {},
                    ),
                ),
            },
            GetFileSystemNodeResult::File(file_node) => GetFileSystemNodeResponse {
                result: Some(
                    golem::workerexecutor::v1::get_file_system_node_response::Result::FileSuccess(
                        golem::workerexecutor::v1::ListFileDataSuccessResponse {
                            file: Some(file_node.into()),
                        },
                    ),
                ),
            },
        };

        Ok(response)
    }

    pub(super) async fn get_file_contents_internal(
        &self,
        request: GetFileContentsRequest,
    ) -> Result<Result<FileReadResponse, FileReadError>, WorkerExecutorError> {
        Self::validate_auth_ctx(&request.auth_ctx)?;
        let path = validate_file_read_path(&request.file_path).and_then(|()| {
            CanonicalFilePath::from_abs_str(&request.file_path)
                .map_err(|_| FileReadError::InvalidTarget)
        });
        let selection = request
            .selection
            .ok_or(FileReadError::InvalidSelection)
            .and_then(FileByteSelection::try_from);
        let (path, selection) = match (path, selection) {
            (Ok(path), Ok(selection)) => (path, selection),
            (Err(error), _) | (_, Err(error)) => {
                return Ok(Err(error));
            }
        };
        let absent = || FileReadResponse {
            head: FileReadHead::Absent,
            body: Box::pin(futures::stream::empty()),
        };
        let worker = match self.get_existing_running(&request).await {
            Ok(worker) => worker,
            Err(WorkerExecutorError::AgentNotFound { .. }) => return Ok(Ok(absent())),
            Err(error) => return Err(error),
        };
        match worker.read_file(path, selection).await {
            Err(FileReadError::Lifecycle) if worker.answers_as_deleted().await => Ok(Ok(absent())),
            result => Ok(result),
        }
    }
}
