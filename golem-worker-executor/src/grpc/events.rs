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
    AgentEvent, AgentStatus, BroadcastStreamRecvError, ConnectWorkerRequest, Context, HasAll,
    InternalWorkerEvent, InterruptKind, InvocationContextStack, Pin, Poll, Response, Status,
    Stream, Timestamp, UsesAllDeps, Worker, WorkerCtx, WorkerEventReceiver, WorkerExecutor,
    WorkerExecutorError, WorkerExecutorImpl, extract_owned_agent_id, extract_principal, golem,
    info, record_closed_grpc_api_active_stream, record_new_grpc_api_active_stream,
};

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static>
    WorkerExecutorImpl<Ctx, Svcs>
{
    pub(super) async fn connect_worker_internal(
        &self,
        request: ConnectWorkerRequest,
    ) -> Result<Response<<Self as WorkerExecutor>::ConnectWorkerStream>, WorkerExecutorError> {
        let owned_agent_id =
            extract_owned_agent_id(&request, |r| &r.agent_id, |r| &r.environment_id)?;

        self.ensure_worker_belongs_to_this_executor(&owned_agent_id)?;
        Self::validate_auth_ctx(&request.auth_ctx)?;

        let principal = extract_principal(&request.principal);

        let metadata = Worker::<Ctx>::get_latest_metadata(self, &owned_agent_id)
            .await?
            .ok_or(WorkerExecutorError::worker_not_found(
                owned_agent_id.agent_id(),
            ))?;

        self.ensure_not_failed(
            &owned_agent_id,
            metadata.agent_mode,
            &metadata.last_known_status,
        )
        .await?;

        if metadata.last_known_status.status != AgentStatus::Interrupted {
            let event_service = Worker::get_or_create_suspended(
                self,
                &owned_agent_id,
                None,
                Vec::new(),
                None,
                None,
                &InvocationContextStack::fresh(),
                principal,
            )
            .await?
            .event_service();

            let receiver = event_service.receiver();

            info!("Client connected");

            Ok(Response::new(WorkerEventStream::new(receiver)))
        } else {
            // We don't want 'connect' to resume interrupted workers
            Err(WorkerExecutorError::Interrupted {
                kind: InterruptKind::Interrupt(Timestamp::now_utc()),
            })
        }
    }
}

pub struct WorkerEventStream {
    inner:
        Pin<Box<dyn Stream<Item = Result<InternalWorkerEvent, BroadcastStreamRecvError>> + Send>>,
}

impl WorkerEventStream {
    pub fn new(receiver: WorkerEventReceiver) -> Self {
        record_new_grpc_api_active_stream();
        WorkerEventStream {
            inner: Box::pin(receiver.to_stream()),
        }
    }
}

impl Drop for WorkerEventStream {
    fn drop(&mut self) {
        record_closed_grpc_api_active_stream();
    }
}

impl Stream for WorkerEventStream {
    type Item = Result<golem::worker::LogEvent, Status>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let WorkerEventStream { inner } = self.get_mut();
        match inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(event))) => {
                Poll::Ready(Some(Ok(AgentEvent::from(event).try_into().unwrap())))
            }
            Poll::Ready(Some(Err(BroadcastStreamRecvError::Lagged(n)))) => {
                Poll::Ready(Some(Ok(AgentEvent::ClientLagged {
                    number_of_missed_messages: n,
                }
                .try_into()
                .unwrap())))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}
