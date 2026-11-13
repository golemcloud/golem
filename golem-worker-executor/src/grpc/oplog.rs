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
    GetOplogEntryRequest, GetOplogEntryResponse, GetOplogRequest, GetOplogResponse, HasAll,
    HasComponentService, HasOplogService, HasWorkerService, OplogIndex, OwnedSemaphorePermit,
    PublicOplogPageLimits, SearchOplogRequest, SearchOplogResponse, Status, Stream, UsesAllDeps,
    WorkerCtx, WorkerExecutorError, WorkerExecutorImpl, extract_owned_agent_id,
    get_public_oplog_entry, get_public_oplog_page, golem, min, oplog_page_cursor, oplog_page_start,
    search_public_oplog_page,
};

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static>
    WorkerExecutorImpl<Ctx, Svcs>
{
    /// Limits of an oplog page that this executor returns to a remote caller. They keep the
    /// page below the gRPC message size limit; `GetOplogEntry` returns the omitted entries. A
    /// page of no entries is refused: its cursor would point to where it started, and a caller
    /// that follows cursors would never end.
    fn oplog_query_limits(
        &self,
        requested_count: u64,
    ) -> Result<PublicOplogPageLimits, WorkerExecutorError> {
        if requested_count == 0 {
            return Err(WorkerExecutorError::invalid_request(
                "the count of an oplog page must be at least 1",
            ));
        }
        let limits = &self.services.config().limits;
        Ok(PublicOplogPageLimits {
            count: min(requested_count as usize, limits.max_oplog_query_pages_size),
            max_bytes: limits.max_oplog_query_page_bytes,
            max_entry_bytes: limits.max_inline_oplog_entry_size,
        })
    }

    pub(super) async fn get_oplog_internal(
        &self,
        request: GetOplogRequest,
    ) -> Result<GetOplogResponse, WorkerExecutorError> {
        let owned_agent_id =
            extract_owned_agent_id(&request, |r| &r.agent_id, |r| &r.environment_id)?;
        self.ensure_worker_belongs_to_this_executor(&owned_agent_id)?;

        let component_service = self.component_service();
        let identity = self
            .worker_service()
            .resolve_agent_identity(&owned_agent_id)
            .await?
            .ok_or_else(|| {
                WorkerExecutorError::invalid_request(format!(
                    "agent {owned_agent_id} does not exist"
                ))
            })?;
        let agent_mode = identity.agent_mode;
        let agent_type_name = &identity.create_parameters().agent_type_name;

        let limits = self.oplog_query_limits(request.count)?;
        let (start, start_component_revision) = oplog_page_start(
            self.oplog_service(),
            &owned_agent_id,
            agent_mode,
            request.cursor.map(Into::into),
            OplogIndex::from_u64(request.from_oplog_index),
            None,
        )
        .await?;
        let chunk = get_public_oplog_page(
            component_service,
            self.oplog_service(),
            &owned_agent_id,
            agent_mode,
            identity.create_parameters().component_id,
            identity.create_parameters().owner_kind,
            agent_type_name.as_ref(),
            start_component_revision,
            start,
            limits,
        )
        .await
        .map_err(WorkerExecutorError::unknown)?;

        let next = oplog_page_cursor(
            chunk.next_oplog_index,
            chunk.current_component_revision,
            chunk.last_index,
            None,
        )
        .map(Into::into);

        Ok(GetOplogResponse {
            result: Some(
                golem::workerexecutor::v1::get_oplog_response::Result::Success(
                    golem::workerexecutor::v1::GetOplogSuccessResponse {
                        entries: chunk.entries,
                        next,
                        first_index_in_chunk: chunk.first_index_in_chunk.into(),
                        last_index: chunk.last_index.into(),
                    },
                ),
            ),
        })
    }

    pub(super) async fn get_oplog_entry_internal(
        &self,
        request: GetOplogEntryRequest,
    ) -> Result<(Vec<u8>, OwnedSemaphorePermit), WorkerExecutorError> {
        let owned_agent_id =
            extract_owned_agent_id(&request, |r| &r.agent_id, |r| &r.environment_id)?;
        self.ensure_worker_belongs_to_this_executor(&owned_agent_id)?;

        let identity = self
            .worker_service()
            .resolve_agent_identity(&owned_agent_id)
            .await?
            .ok_or_else(|| {
                WorkerExecutorError::invalid_request(format!(
                    "agent {owned_agent_id} does not exist"
                ))
            })?;
        let agent_type_name = &identity.create_parameters().agent_type_name;

        let component_revision = request
            .component_revision
            .map(golem_common::model::component::ComponentRevision::try_from)
            .transpose()
            .map_err(|e| {
                WorkerExecutorError::invalid_request(format!("invalid component_revision: {e}"))
            })?;

        let admission = self
            .oplog_entry_reads
            .clone()
            .acquire_owned()
            .await
            .expect("the oplog entry read semaphore is never closed");
        let entry = get_public_oplog_entry(
            self.component_service(),
            self.oplog_service(),
            &owned_agent_id,
            identity.agent_mode,
            identity.create_parameters().component_id,
            identity.create_parameters().owner_kind,
            agent_type_name.as_ref(),
            OplogIndex::from_u64(request.oplog_index),
            component_revision,
        )
        .await?;
        let entry: golem::worker::OplogEntryWithIndex =
            entry.try_into().map_err(WorkerExecutorError::unknown)?;
        Ok((prost::Message::encode_to_vec(&entry), admission))
    }

    pub(super) async fn search_oplog_internal(
        &self,
        request: SearchOplogRequest,
    ) -> Result<SearchOplogResponse, WorkerExecutorError> {
        let owned_agent_id =
            extract_owned_agent_id(&request, |r| &r.agent_id, |r| &r.environment_id)?;

        self.ensure_worker_belongs_to_this_executor(&owned_agent_id)?;

        let component_service = self.component_service();
        let identity = self
            .worker_service()
            .resolve_agent_identity(&owned_agent_id)
            .await?
            .ok_or_else(|| {
                WorkerExecutorError::invalid_request(format!(
                    "agent {owned_agent_id} does not exist"
                ))
            })?;
        let agent_mode = identity.agent_mode;
        let agent_type_name = &identity.create_parameters().agent_type_name;

        let limits = self.oplog_query_limits(request.count)?;
        let (start, start_component_revision) = oplog_page_start(
            self.oplog_service(),
            &owned_agent_id,
            agent_mode,
            request.cursor.map(Into::into),
            OplogIndex::INITIAL,
            Some(&request.query),
        )
        .await?;
        let chunk = search_public_oplog_page(
            component_service,
            self.oplog_service(),
            &owned_agent_id,
            agent_mode,
            identity.create_parameters().component_id,
            identity.create_parameters().owner_kind,
            agent_type_name.as_ref(),
            start_component_revision,
            start,
            limits,
            &request.query,
            self.services.config().limits.scan_budget,
        )
        .await
        .map_err(WorkerExecutorError::unknown)?;

        let next = oplog_page_cursor(
            chunk.next_oplog_index,
            chunk.current_component_revision,
            chunk.last_index,
            Some(&request.query),
        )
        .map(Into::into);

        Ok(SearchOplogResponse {
            result: Some(
                golem::workerexecutor::v1::search_oplog_response::Result::Success(
                    golem::workerexecutor::v1::SearchOplogSuccessResponse {
                        entries: chunk.entries,
                        next,
                        last_index: chunk.last_index.into(),
                    },
                ),
            ),
        })
    }
}

/// The size of the chunks in which `GetOplogEntry` sends an encoded oplog entry.
pub(super) const OPLOG_ENTRY_CHUNK_SIZE: usize = 4 * 1024 * 1024;

/// Splits an encoded oplog entry into `GetOplogEntry` chunk frames. The stream owns the encoded
/// entry and copies a chunk out of it only when the frame is polled. It also owns `admission`,
/// which it releases when it is dropped: after the last frame, or when the caller cancels.
pub(super) fn oplog_entry_frames<A: Send + 'static>(
    encoded: Vec<u8>,
    chunk_size: usize,
    admission: A,
) -> impl Stream<Item = Result<GetOplogEntryResponse, Status>> + Send + 'static {
    use golem::workerexecutor::v1::get_oplog_entry_response::Result as Frame;
    let len = encoded.len();
    futures::stream::iter((0..len).step_by(chunk_size).map(move |start| {
        let _admission = &admission;
        let end = (start + chunk_size).min(len);
        Ok(GetOplogEntryResponse {
            result: Some(Frame::Chunk(encoded[start..end].to_vec())),
        })
    }))
}
