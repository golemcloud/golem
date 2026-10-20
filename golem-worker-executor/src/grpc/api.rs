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
    ActivatePluginRequest, ActivatePluginResponse, AppendToStreamSlotRequest,
    AppendToStreamSlotResponse, AuthCtx, CancelInvocationRequest, CancelInvocationResponse,
    ComponentId, DeactivatePluginRequest, DeactivatePluginResponse, DeliverCardTransferRequest,
    DeliverCardTransferResponse, DurableStreamAttachmentControlRequest,
    DurableStreamAttachmentControlResponse, DurableStreamRemoteError,
    DurableStreamSegmentReadRequest, DurableStreamSegmentReadResponse, ForkStreamSlotRequest,
    ForkStreamSlotResponse, ForkWorkerRequest, ForkWorkerResponse, GetAgentWalletRequest,
    GetAgentWalletResponse, GetFileContentsRequest, GetFileContentsResponse,
    GetFileSystemNodeRequest, GetFileSystemNodeResponse, GetOplogEntryRequest,
    GetOplogEntryResponse, GetOplogRequest, GetOplogResponse, GetRunningWorkersMetadataRequest,
    GetRunningWorkersMetadataResponse, GetWorkersMetadataRequest, GetWorkersMetadataResponse,
    HasAll, Instrument, InvocationRequest, OPLOG_ENTRY_CHUNK_SIZE, Pin, ProcessOplogEntriesRequest,
    ProcessOplogEntriesResponse, ReadStreamSlotRequest, ReadStreamSlotResponse, Request,
    ResolveRevertLastInvocationsRequest, ResolveRevertLastInvocationsResponse, Response,
    ResponseResult, ResponseStream, RevertWorkerRequest, RevertWorkerResponse, SearchOplogRequest,
    SearchOplogResponse, Status, Stream, StreamExt, UpdateWorkerRequest, UpdateWorkerResponse,
    UsesAllDeps, WorkerCtx, WorkerExecutor, WorkerExecutorError, WorkerExecutorImpl,
    deliver_card_transfer_response, durable_stream_attachment_control_response,
    durable_stream_segment_read_response, extract_owned_agent_id, golem, invocation_session,
    oplog_entry_frames, process_oplog_entries_response, proto_agent_id_string,
    proto_idempotency_key_string, proto_promise_id_string, recorded_grpc_api_request,
    resolve_revert_last_invocations_response,
};

#[tonic::async_trait]
impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static>
    WorkerExecutor for WorkerExecutorImpl<Ctx, Svcs>
{
    async fn create_worker(
        &self,
        request: Request<golem::workerexecutor::v1::CreateWorkerRequest>,
    ) -> Result<Response<golem::workerexecutor::v1::CreateWorkerResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "create_worker",
            agent_id = proto_agent_id_string(&request.agent_id),
        );

        match self
            .create_worker_internal(request, true)
            .instrument(record.span.clone())
            .await
        {
            Ok(fingerprint) => record.succeed(Ok(Response::new(
                golem::workerexecutor::v1::CreateWorkerResponse {
                    result: Some(
                        golem::workerexecutor::v1::create_worker_response::Result::Success(
                            golem::workerexecutor::v1::CreateWorkerSuccessResponse {
                                instance_id: Some(fingerprint.0.into()),
                            },
                        ),
                    ),
                },
            ))),
            Err(mut err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::CreateWorkerResponse {
                        result: Some(
                            golem::workerexecutor::v1::create_worker_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &mut err,
            ),
        }
    }

    async fn prepare_worker(
        &self,
        request: Request<golem::workerexecutor::v1::CreateWorkerRequest>,
    ) -> Result<Response<golem::workerexecutor::v1::CreateWorkerResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "prepare_worker",
            agent_id = proto_agent_id_string(&request.agent_id),
        );

        match self
            .create_worker_internal(request, false)
            .instrument(record.span.clone())
            .await
        {
            Ok(fingerprint) => record.succeed(Ok(Response::new(
                golem::workerexecutor::v1::CreateWorkerResponse {
                    result: Some(
                        golem::workerexecutor::v1::create_worker_response::Result::Success(
                            golem::workerexecutor::v1::CreateWorkerSuccessResponse {
                                instance_id: Some(fingerprint.0.into()),
                            },
                        ),
                    ),
                },
            ))),
            Err(mut err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::CreateWorkerResponse {
                        result: Some(
                            golem::workerexecutor::v1::create_worker_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &mut err,
            ),
        }
    }

    type ConnectWorkerStream = ResponseStream;

    async fn connect_worker(
        &self,
        request: Request<golem::workerexecutor::v1::ConnectWorkerRequest>,
    ) -> ResponseResult<Self::ConnectWorkerStream> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "connect_worker",
            agent_id = proto_agent_id_string(&request.agent_id),
        );

        match self
            .connect_worker_internal(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(stream) => Ok(record.succeed(stream)),
            Err(mut err) => Err(record.fail(err.clone().into(), &mut err)),
        }
    }

    async fn delete_worker(
        &self,
        request: Request<golem::workerexecutor::v1::DeleteWorkerRequest>,
    ) -> Result<Response<golem::workerexecutor::v1::DeleteWorkerResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "delete_worker",
            agent_id = proto_agent_id_string(&request.agent_id)
        );

        match self
            .delete_worker_internal(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(Ok(Response::new(
                golem::workerexecutor::v1::DeleteWorkerResponse {
                    result: Some(
                        golem::workerexecutor::v1::delete_worker_response::Result::Success(
                            golem_schema::proto::golem::common::Empty {},
                        ),
                    ),
                },
            ))),
            Err(mut err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::DeleteWorkerResponse {
                        result: Some(
                            golem::workerexecutor::v1::delete_worker_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &mut err,
            ),
        }
    }

    async fn complete_promise(
        &self,
        request: Request<golem::workerexecutor::v1::CompletePromiseRequest>,
    ) -> Result<Response<golem::workerexecutor::v1::CompletePromiseResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "complete_promise",
            promise_id = proto_promise_id_string(&request.promise_id)
        );

        match self
            .complete_promise_internal(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(success) => record.succeed(Ok(Response::new(
                golem::workerexecutor::v1::CompletePromiseResponse {
                    result: Some(
                        golem::workerexecutor::v1::complete_promise_response::Result::Success(
                            success,
                        ),
                    ),
                },
            ))),
            Err(mut err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::CompletePromiseResponse {
                        result: Some(
                            golem::workerexecutor::v1::complete_promise_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &mut err,
            ),
        }
    }

    async fn interrupt_worker(
        &self,
        request: Request<golem::workerexecutor::v1::InterruptWorkerRequest>,
    ) -> Result<Response<golem::workerexecutor::v1::InterruptWorkerResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "interrupt_worker",
            agent_id = proto_agent_id_string(&request.agent_id),
        );

        match self
            .interrupt_worker_internal(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(Ok(Response::new(
                golem::workerexecutor::v1::InterruptWorkerResponse {
                    result: Some(
                        golem::workerexecutor::v1::interrupt_worker_response::Result::Success(
                            golem_schema::proto::golem::common::Empty {},
                        ),
                    ),
                },
            ))),
            Err(mut err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::InterruptWorkerResponse {
                        result: Some(
                            golem::workerexecutor::v1::interrupt_worker_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &mut err,
            ),
        }
    }

    async fn revoke_shards(
        &self,
        request: Request<golem::workerexecutor::v1::RevokeShardsRequest>,
    ) -> Result<Response<golem::workerexecutor::v1::RevokeShardsResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!("revoke_shards",);

        match self
            .revoke_shards_internal(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(Ok(Response::new(
                golem::workerexecutor::v1::RevokeShardsResponse {
                    result: Some(
                        golem::workerexecutor::v1::revoke_shards_response::Result::Success(
                            golem_schema::proto::golem::common::Empty {},
                        ),
                    ),
                },
            ))),
            Err(mut err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::RevokeShardsResponse {
                        result: Some(
                            golem::workerexecutor::v1::revoke_shards_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &mut err,
            ),
        }
    }

    async fn assign_shards(
        &self,
        request: Request<golem::workerexecutor::v1::AssignShardsRequest>,
    ) -> Result<Response<golem::workerexecutor::v1::AssignShardsResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!("assign_shards",);

        match self
            .assign_shards_internal(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(Ok(Response::new(
                golem::workerexecutor::v1::AssignShardsResponse {
                    result: Some(
                        golem::workerexecutor::v1::assign_shards_response::Result::Success(
                            golem_schema::proto::golem::common::Empty {},
                        ),
                    ),
                },
            ))),
            Err(mut err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::AssignShardsResponse {
                        result: Some(
                            golem::workerexecutor::v1::assign_shards_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &mut err,
            ),
        }
    }

    async fn get_agent_metadata(
        &self,
        request: Request<golem::workerexecutor::v1::GetAgentMetadataRequest>,
    ) -> Result<Response<golem::workerexecutor::v1::GetAgentMetadataResponse>, Status> {
        let request = request.into_inner();

        let record = recorded_grpc_api_request!(
            "get_agent_metadata",
            agent_id = proto_agent_id_string(&request.agent_id)
        );

        let result = self
            .get_agent_metadata_internal(request)
            .instrument(record.span.clone())
            .await;
        match result {
            Ok(result) => record.succeed(Ok(Response::new(
                golem::workerexecutor::v1::GetAgentMetadataResponse {
                    result: Some(
                        golem::workerexecutor::v1::get_agent_metadata_response::Result::Success(
                            result,
                        ),
                    ),
                },
            ))),
            Err(err @ WorkerExecutorError::AgentNotFound { .. }) => record.succeed(Ok(
                Response::new(golem::workerexecutor::v1::GetAgentMetadataResponse {
                    result: Some(
                        golem::workerexecutor::v1::get_agent_metadata_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                }),
            )),
            Err(mut err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::GetAgentMetadataResponse {
                        result: Some(
                            golem::workerexecutor::v1::get_agent_metadata_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &mut err,
            ),
        }
    }

    async fn resume_worker(
        &self,
        request: Request<golem::workerexecutor::v1::ResumeWorkerRequest>,
    ) -> Result<Response<golem::workerexecutor::v1::ResumeWorkerResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "resume_worker",
            agent_id = proto_agent_id_string(&request.agent_id),
        );

        match self
            .resume_worker_internal(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(_) => record.succeed(Ok(Response::new(
                golem::workerexecutor::v1::ResumeWorkerResponse {
                    result: Some(
                        golem::workerexecutor::v1::resume_worker_response::Result::Success(
                            golem_schema::proto::golem::common::Empty {},
                        ),
                    ),
                },
            ))),
            Err(mut err) => record.fail(
                Ok(Response::new(
                    golem::workerexecutor::v1::ResumeWorkerResponse {
                        result: Some(
                            golem::workerexecutor::v1::resume_worker_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &mut err,
            ),
        }
    }

    async fn get_running_workers_metadata(
        &self,
        request: Request<GetRunningWorkersMetadataRequest>,
    ) -> Result<Response<GetRunningWorkersMetadataResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_running_workers_metadata",
            component_id = ComponentId::render_proto(request.component_id),
        );

        let result = self
            .get_running_workers_metadata_internal(request)
            .instrument(record.span.clone())
            .await;
        match result {
            Ok(workers) => record.succeed(Ok(Response::new(
                golem::workerexecutor::v1::GetRunningWorkersMetadataResponse {
                    result: Some(
                        golem::workerexecutor::v1::get_running_workers_metadata_response::Result::Success(
                            golem::workerexecutor::v1::GetRunningWorkersMetadataSuccessResponse {
                                workers
                            }
                        ),
                    ),
                },
            ))),
            Err(mut err) => record.fail(
                Ok(Response::new(
                    GetRunningWorkersMetadataResponse {
                        result: Some(
                            golem::workerexecutor::v1::get_running_workers_metadata_response::Result::Failure(
                                err.clone().into(),
                            ),
                        ),
                    },
                )),
                &mut err,
            ),
        }
    }

    async fn get_workers_metadata(
        &self,
        request: Request<GetWorkersMetadataRequest>,
    ) -> Result<Response<GetWorkersMetadataResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_workers_metadata",
            component_id = ComponentId::render_proto(request.component_id),
        );

        let result = self
            .get_workers_metadata_internal(request)
            .instrument(record.span.clone())
            .await;
        match result {
            Ok((cursor, workers)) => {
                record.succeed(Ok(Response::new(GetWorkersMetadataResponse {
                    result: Some(
                        golem::workerexecutor::v1::get_workers_metadata_response::Result::Success(
                            golem::workerexecutor::v1::GetWorkersMetadataSuccessResponse {
                                workers,
                                cursor,
                            },
                        ),
                    ),
                })))
            }
            Err(mut err) => record.fail(
                Ok(Response::new(GetWorkersMetadataResponse {
                    result: Some(
                        golem::workerexecutor::v1::get_workers_metadata_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &mut err,
            ),
        }
    }

    async fn update_worker(
        &self,
        request: Request<UpdateWorkerRequest>,
    ) -> Result<Response<UpdateWorkerResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "update_worker",
            agent_id = proto_agent_id_string(&request.agent_id),
            target_revision = request.target_revision,
        );

        match self
            .update_worker_internal(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(update_attempt_index) => record.succeed(Ok(Response::new(UpdateWorkerResponse {
                result: Some(
                    golem::workerexecutor::v1::update_worker_response::Result::Success(
                        update_attempt_index.into(),
                    ),
                ),
            }))),
            Err(mut err) => record.fail(
                Ok(Response::new(UpdateWorkerResponse {
                    result: Some(
                        golem::workerexecutor::v1::update_worker_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &mut err,
            ),
        }
    }

    async fn get_oplog(
        &self,
        request: Request<GetOplogRequest>,
    ) -> Result<Response<GetOplogResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_oplog",
            agent_id = proto_agent_id_string(&request.agent_id),
        );

        let result = self
            .get_oplog_internal(request)
            .instrument(record.span.clone())
            .await;
        match result {
            Ok(response) => record.succeed(Ok(Response::new(response))),
            Err(mut err) => record.fail(
                Ok(Response::new(GetOplogResponse {
                    result: Some(
                        golem::workerexecutor::v1::get_oplog_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &mut err,
            ),
        }
    }

    type GetOplogEntryStream =
        Pin<Box<dyn Stream<Item = Result<GetOplogEntryResponse, Status>> + Send + 'static>>;

    async fn get_oplog_entry(
        &self,
        request: Request<GetOplogEntryRequest>,
    ) -> ResponseResult<Self::GetOplogEntryStream> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_oplog_entry",
            agent_id = proto_agent_id_string(&request.agent_id),
            oplog_index = request.oplog_index,
        );

        let result = self
            .get_oplog_entry_internal(request)
            .instrument(record.span.clone())
            .await;

        use golem::workerexecutor::v1::get_oplog_entry_response::Result as Frame;
        let stream: Self::GetOplogEntryStream = match result {
            Ok((encoded, admission)) => record.succeed(Box::pin(oplog_entry_frames(
                encoded,
                OPLOG_ENTRY_CHUNK_SIZE,
                admission,
            ))),
            Err(mut err) => {
                let stream: Self::GetOplogEntryStream =
                    Box::pin(futures::stream::iter([Ok(GetOplogEntryResponse {
                        result: Some(Frame::Failure(err.clone().into())),
                    })]));
                record.fail(stream, &mut err)
            }
        };
        Ok(Response::new(stream))
    }

    async fn search_oplog(
        &self,
        request: Request<SearchOplogRequest>,
    ) -> Result<Response<SearchOplogResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "search_oplog",
            agent_id = proto_agent_id_string(&request.agent_id),
        );

        let result = self
            .search_oplog_internal(request)
            .instrument(record.span.clone())
            .await;
        match result {
            Ok(response) => record.succeed(Ok(Response::new(response))),
            Err(mut err) => record.fail(
                Ok(Response::new(SearchOplogResponse {
                    result: Some(
                        golem::workerexecutor::v1::search_oplog_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &mut err,
            ),
        }
    }

    async fn fork_worker(
        &self,
        request: Request<ForkWorkerRequest>,
    ) -> Result<Response<ForkWorkerResponse>, Status> {
        let request = request.into_inner();

        let record = recorded_grpc_api_request!(
            "fork_worker",
            source_agent_id = proto_agent_id_string(&request.source_agent_id),
            target_agent_id = proto_agent_id_string(&request.target_agent_id),
        );

        let result = self
            .fork_worker_internal(request)
            .instrument(record.span.clone())
            .await;

        match result {
            Ok(_) => record.succeed(Ok(Response::new(ForkWorkerResponse {
                result: Some(
                    golem::workerexecutor::v1::fork_worker_response::Result::Success(
                        golem_schema::proto::golem::common::Empty {},
                    ),
                ),
            }))),
            Err(mut err) => record.fail(
                Ok(Response::new(ForkWorkerResponse {
                    result: Some(
                        golem::workerexecutor::v1::fork_worker_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &mut err,
            ),
        }
    }

    async fn revert_worker(
        &self,
        request: Request<RevertWorkerRequest>,
    ) -> Result<Response<RevertWorkerResponse>, Status> {
        let request = request.into_inner();

        let record = recorded_grpc_api_request!(
            "revert_worker",
            agent_id = proto_agent_id_string(&request.agent_id),
        );

        let result = self
            .revert_worker_internal(request)
            .instrument(record.span.clone())
            .await;

        match result {
            Ok(_) => record.succeed(Ok(Response::new(RevertWorkerResponse {
                result: Some(
                    golem::workerexecutor::v1::revert_worker_response::Result::Success(
                        golem_schema::proto::golem::common::Empty {},
                    ),
                ),
            }))),
            Err(mut err) => record.fail(
                Ok(Response::new(RevertWorkerResponse {
                    result: Some(
                        golem::workerexecutor::v1::revert_worker_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &mut err,
            ),
        }
    }

    async fn resolve_revert_last_invocations(
        &self,
        request: Request<ResolveRevertLastInvocationsRequest>,
    ) -> Result<Response<ResolveRevertLastInvocationsResponse>, Status> {
        let request = request.into_inner();

        let record = recorded_grpc_api_request!(
            "resolve_revert_last_invocations",
            agent_id = proto_agent_id_string(&request.agent_id),
        );

        let result = self
            .resolve_revert_last_invocations_internal(request)
            .instrument(record.span.clone())
            .await;

        match result {
            Ok(resolved) => {
                record.succeed(Ok(Response::new(ResolveRevertLastInvocationsResponse {
                    result: Some(resolve_revert_last_invocations_response::Result::Success(
                        golem::common::ResolvedRevert {
                            last_oplog_index: resolved.last_oplog_index.as_u64(),
                            observed_oplog_index: resolved.observed_oplog_index.as_u64(),
                        },
                    )),
                })))
            }
            Err(mut err) => record.fail(
                Ok(Response::new(ResolveRevertLastInvocationsResponse {
                    result: Some(resolve_revert_last_invocations_response::Result::Failure(
                        err.clone().into(),
                    )),
                })),
                &mut err,
            ),
        }
    }

    async fn cancel_invocation(
        &self,
        request: Request<CancelInvocationRequest>,
    ) -> Result<Response<CancelInvocationResponse>, Status> {
        let request = request.into_inner();

        let record = recorded_grpc_api_request!(
            "cancel_invocation",
            agent_id = proto_agent_id_string(&request.agent_id),
            idempotency_key = proto_idempotency_key_string(&request.idempotency_key),
        );

        let result = self
            .cancel_invocation_internal(request)
            .instrument(record.span.clone())
            .await;

        match result {
            Ok(canceled) => record.succeed(Ok(Response::new(CancelInvocationResponse {
                result: Some(
                    golem::workerexecutor::v1::cancel_invocation_response::Result::Success(
                        canceled,
                    ),
                ),
            }))),
            Err(mut err) => record.fail(
                Ok(Response::new(CancelInvocationResponse {
                    result: Some(
                        golem::workerexecutor::v1::cancel_invocation_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &mut err,
            ),
        }
    }

    async fn get_file_system_node(
        &self,
        request: Request<GetFileSystemNodeRequest>,
    ) -> ResponseResult<GetFileSystemNodeResponse> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_file_system_node",
            agent_id = proto_agent_id_string(&request.agent_id),
            path = request.path,
        );

        let result = self
            .get_file_system_node_internal(request)
            .instrument(record.span.clone())
            .await;
        match result {
            Ok(response) => record.succeed(Ok(Response::new(response))),
            Err(mut err) => record.fail(
                Ok(Response::new(GetFileSystemNodeResponse {
                    result: Some(
                        golem::workerexecutor::v1::get_file_system_node_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &mut err,
            ),
        }
    }

    async fn get_agent_wallet(
        &self,
        request: Request<GetAgentWalletRequest>,
    ) -> ResponseResult<GetAgentWalletResponse> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_agent_wallet",
            agent_id = proto_agent_id_string(&request.agent_id),
        );

        let result = self
            .get_agent_wallet_internal(request)
            .instrument(record.span.clone())
            .await;
        match result {
            Ok(response) => record.succeed(Ok(Response::new(response))),
            Err(mut err) => record.fail(
                Ok(Response::new(GetAgentWalletResponse {
                    result: Some(
                        golem::workerexecutor::v1::get_agent_wallet_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &mut err,
            ),
        }
    }

    type GetFileContentsStream =
        Pin<Box<dyn Stream<Item = Result<GetFileContentsResponse, Status>> + Send + 'static>>;

    async fn get_file_contents(
        &self,
        request: Request<GetFileContentsRequest>,
    ) -> ResponseResult<Self::GetFileContentsStream> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_file_contents",
            agent_id = proto_agent_id_string(&request.agent_id),
        );

        let result = self
            .get_file_contents_internal(request)
            .instrument(record.span.clone())
            .await;

        use golem::workerexecutor::v1::get_file_contents_response::Result as Frame;
        let stream: Self::GetFileContentsStream = match result {
            Ok(Ok(response)) => {
                let head = futures::stream::iter([Ok(GetFileContentsResponse {
                    result: Some(Frame::Header(response.head.into())),
                })]);
                let body = response.body.map(|item| {
                    Ok(GetFileContentsResponse {
                        result: Some(match item {
                            Ok(bytes) => Frame::Success(bytes.into()),
                            Err(error) => {
                                Frame::ReadFailure(golem::worker::FileReadError::from(error) as i32)
                            }
                        }),
                    })
                });
                record.succeed(Box::pin(head.chain(body)))
            }
            Ok(Err(mut error)) => {
                let stream: Self::GetFileContentsStream =
                    Box::pin(futures::stream::iter([Ok(GetFileContentsResponse {
                        result: Some(Frame::ReadFailure(
                            golem::worker::FileReadError::from(error) as i32,
                        )),
                    })]));
                record.fail(stream, &mut error)
            }
            Err(mut err) => {
                let res = GetFileContentsResponse {
                    result: Some(
                        golem::workerexecutor::v1::get_file_contents_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                };

                let err_stream: Self::GetFileContentsStream =
                    Box::pin(tokio_stream::iter(vec![Ok(res)]));

                record.fail(err_stream, &mut err)
            }
        };
        Ok(Response::new(stream))
    }

    async fn activate_plugin(
        &self,
        request: Request<ActivatePluginRequest>,
    ) -> Result<Response<ActivatePluginResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "activate_plugin",
            agent_id = proto_agent_id_string(&request.agent_id),
            plugin_priority = request.plugin_priority
        );

        let result = self
            .activate_plugin_internal(request)
            .instrument(record.span.clone())
            .await;

        match result {
            Ok(_) => record.succeed(Ok(Response::new(ActivatePluginResponse {
                result: Some(
                    golem::workerexecutor::v1::activate_plugin_response::Result::Success(
                        golem_schema::proto::golem::common::Empty {},
                    ),
                ),
            }))),
            Err(mut err) => record.fail(
                Ok(Response::new(ActivatePluginResponse {
                    result: Some(
                        golem::workerexecutor::v1::activate_plugin_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &mut err,
            ),
        }
    }

    async fn deactivate_plugin(
        &self,
        request: Request<DeactivatePluginRequest>,
    ) -> Result<Response<DeactivatePluginResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "deactivate_plugin",
            agent_id = proto_agent_id_string(&request.agent_id),
            plugin_priority = request.plugin_priority
        );

        let result = self
            .deactivate_plugin_internal(request)
            .instrument(record.span.clone())
            .await;

        match result {
            Ok(_) => record.succeed(Ok(Response::new(DeactivatePluginResponse {
                result: Some(
                    golem::workerexecutor::v1::deactivate_plugin_response::Result::Success(
                        golem_schema::proto::golem::common::Empty {},
                    ),
                ),
            }))),
            Err(mut err) => record.fail(
                Ok(Response::new(DeactivatePluginResponse {
                    result: Some(
                        golem::workerexecutor::v1::deactivate_plugin_response::Result::Failure(
                            err.clone().into(),
                        ),
                    ),
                })),
                &mut err,
            ),
        }
    }

    type InvokeAgentSessionStream = invocation_session::InvocationSessionStream;

    async fn invoke_agent_session(
        &self,
        request: Request<tonic::Streaming<InvocationRequest>>,
    ) -> ResponseResult<Self::InvokeAgentSessionStream> {
        invocation_session::invoke_agent_session(self, request).await
    }

    async fn control_durable_stream_attachment(
        &self,
        request: Request<DurableStreamAttachmentControlRequest>,
    ) -> ResponseResult<DurableStreamAttachmentControlResponse> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "control_durable_stream_attachment",
            agent_id = proto_agent_id_string(&request.producer_agent_id),
        );
        match self
            .control_durable_stream_attachment_internal(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(result) => {
                record.succeed(Ok(Response::new(DurableStreamAttachmentControlResponse {
                    result: Some(result),
                })))
            }
            Err(DurableStreamRemoteError::Unavailable) => {
                let message = "durable stream producer is unavailable";
                record.fail(
                    Err(Status::unavailable(message)),
                    &mut WorkerExecutorError::runtime(message),
                )
            }
            Err(DurableStreamRemoteError::Other(mut error)) => record.fail(
                Ok(Response::new(DurableStreamAttachmentControlResponse {
                    result: Some(durable_stream_attachment_control_response::Result::Failure(
                        error.clone().into(),
                    )),
                })),
                &mut error,
            ),
        }
    }

    async fn read_durable_stream_segment(
        &self,
        request: Request<DurableStreamSegmentReadRequest>,
    ) -> ResponseResult<DurableStreamSegmentReadResponse> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "read_durable_stream_segment",
            agent_id = proto_agent_id_string(&request.producer_agent_id),
        );
        match self
            .read_durable_stream_segment_internal(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(payload) => record.succeed(Ok(Response::new(DurableStreamSegmentReadResponse {
                result: Some(durable_stream_segment_read_response::Result::Payload(
                    payload,
                )),
            }))),
            Err(DurableStreamRemoteError::Unavailable) => {
                let message = "durable stream producer is unavailable";
                record.fail(
                    Err(Status::unavailable(message)),
                    &mut WorkerExecutorError::runtime(message),
                )
            }
            Err(DurableStreamRemoteError::Other(mut error)) => record.fail(
                Ok(Response::new(DurableStreamSegmentReadResponse {
                    result: Some(durable_stream_segment_read_response::Result::Failure(
                        error.clone().into(),
                    )),
                })),
                &mut error,
            ),
        }
    }

    async fn create_stream_session(
        &self,
        request: Request<golem::workerexecutor::v1::CreateStreamSessionRequest>,
    ) -> ResponseResult<golem::workerexecutor::v1::CreateStreamSessionResponse> {
        use golem::workerexecutor::v1::create_stream_session_response::Result as Outcome;
        let result = self
            .create_stream_session_internal(request.into_inner())
            .await;
        Ok(Response::new(
            golem::workerexecutor::v1::CreateStreamSessionResponse {
                result: Some(match result {
                    Ok(success) => Outcome::Success(success),
                    Err(error) => Outcome::Failure(error.into()),
                }),
            },
        ))
    }

    type ReadStreamSlotStream =
        Pin<Box<dyn Stream<Item = Result<ReadStreamSlotResponse, Status>> + Send + 'static>>;

    async fn read_stream_slot(
        &self,
        request: Request<ReadStreamSlotRequest>,
    ) -> ResponseResult<Self::ReadStreamSlotStream> {
        use golem::workerexecutor::v1::read_stream_slot_response::Result as Outcome;
        let request = request.into_inner();
        let worker = match extract_owned_agent_id(&request, |r| &r.agent_id, |r| &r.environment_id)
        {
            Ok(id) => self.stream_slot_worker(&id, request.auth_ctx.clone()).await,
            Err(error) => Err(error.into()),
        };
        let result = match worker {
            Ok((worker, _response_lease)) => match request.try_into() {
                Ok(request) => worker.read_stream_slot(request).await,
                Err(error) => Err(DurableStreamRemoteError::Other(error)),
            },
            Err(error) => Err(error),
        };
        let result = match result {
            Ok(Some(value)) => Outcome::Success(value.into()),
            Ok(None) => Outcome::NotFound(golem_schema::proto::golem::common::Empty {}),
            Err(DurableStreamRemoteError::Other(error)) => Outcome::Failure(error.into()),
            Err(DurableStreamRemoteError::Unavailable) => {
                return Err(Status::unavailable(
                    "durable stream producer is unavailable",
                ));
            }
        };
        Ok(Response::new(Box::pin(futures::stream::once(async move {
            Ok(ReadStreamSlotResponse {
                result: Some(result),
            })
        }))))
    }

    async fn append_to_stream_slot(
        &self,
        request: Request<AppendToStreamSlotRequest>,
    ) -> ResponseResult<AppendToStreamSlotResponse> {
        let request = request.into_inner();
        let result = async {
            let id = extract_owned_agent_id(&request, |r| &r.agent_id, |r| &r.environment_id)?;
            let auth_ctx = request.auth_ctx.clone();
            let (worker, _response_lease) = self.stream_slot_worker(&id, auth_ctx).await?;
            let outcome = worker.append_to_stream_slot(request.into()).await?;
            Ok::<_, DurableStreamRemoteError<WorkerExecutorError>>(
                AppendToStreamSlotResponse::from(outcome),
            )
        }
        .await;
        match result {
            Ok(response) => Ok(Response::new(response)),
            Err(DurableStreamRemoteError::Unavailable) => Err(Status::unavailable(
                "durable stream producer is unavailable",
            )),
            Err(DurableStreamRemoteError::Other(error)) => {
                Ok(Response::new(AppendToStreamSlotResponse {
                    result: Some(
                        golem::workerexecutor::v1::append_to_stream_slot_response::Result::Failure(
                            error.into(),
                        ),
                    ),
                    invocation_key: None,
                    expiry_policy: None,
                    expiry_deadline_millis: None,
                    stream_head_offset: Vec::new(),
                    stream_closed: None,
                }))
            }
        }
    }

    async fn fork_stream_slot(
        &self,
        request: Request<ForkStreamSlotRequest>,
    ) -> ResponseResult<ForkStreamSlotResponse> {
        let request = request.into_inner();
        let auth_ctx: AuthCtx = request
            .auth_ctx
            .clone()
            .ok_or_else(|| Status::permission_denied("fork stream slot is system-only"))?
            .try_into()
            .map_err(Status::permission_denied)?;
        if auth_ctx != AuthCtx::System {
            return Err(Status::permission_denied("fork stream slot is system-only"));
        }
        Ok(Response::new(
            self.services
                .worker_fork_service()
                .fork_stream_slot(request)
                .await,
        ))
    }

    async fn process_oplog_entries(
        &self,
        request: Request<ProcessOplogEntriesRequest>,
    ) -> ResponseResult<ProcessOplogEntriesResponse> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "process_oplog_entries",
            agent_id = proto_agent_id_string(&request.agent_id),
        );

        let result = self
            .process_oplog_entries_internal(request)
            .instrument(record.span.clone())
            .await;

        match result {
            Ok(_) => record.succeed(Ok(Response::new(ProcessOplogEntriesResponse {
                result: Some(process_oplog_entries_response::Result::Success(
                    golem_schema::proto::golem::common::Empty {},
                )),
            }))),
            Err(mut err) => record.fail(
                Ok(Response::new(ProcessOplogEntriesResponse {
                    result: Some(process_oplog_entries_response::Result::Failure(
                        err.clone().into(),
                    )),
                })),
                &mut err,
            ),
        }
    }

    async fn deliver_card_transfer(
        &self,
        request: Request<DeliverCardTransferRequest>,
    ) -> ResponseResult<DeliverCardTransferResponse> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "deliver_card_transfer",
            agent_id = proto_agent_id_string(&request.target_agent_id),
        );

        let result = self
            .deliver_card_transfer_internal(request)
            .instrument(record.span.clone())
            .await;

        match result {
            Ok(()) => record.succeed(Ok(Response::new(DeliverCardTransferResponse {
                result: Some(deliver_card_transfer_response::Result::Success(
                    golem_schema::proto::golem::common::Empty {},
                )),
            }))),
            Err(mut error) => record.fail(
                Ok(Response::new(DeliverCardTransferResponse {
                    result: Some(deliver_card_transfer_response::Result::Failure(
                        error.clone().into(),
                    )),
                })),
                &mut error,
            ),
        }
    }
}
