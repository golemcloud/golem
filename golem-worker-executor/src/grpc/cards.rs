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
    AuthCtx, CanStartWorker, CardId, DeliverCardTransferRequest, GetAgentWalletRequest,
    GetAgentWalletResponse, GetAgentWalletSuccess, HasAll, HasComponentService,
    InvocationFreshnessDisposition, PERMISSION_CARD_INSTALL_RECIPIENT_MISMATCH, ParsedAgentId,
    StoredCard, UsesAllDeps, Worker, WorkerCtx, WorkerExecutorError, WorkerExecutorImpl,
    agent_monomorphization_context, card_matches_agent_recipient, extract_owned_agent_id, golem,
};

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static>
    WorkerExecutorImpl<Ctx, Svcs>
{
    pub(super) async fn get_agent_wallet_internal(
        &self,
        request: GetAgentWalletRequest,
    ) -> Result<GetAgentWalletResponse, WorkerExecutorError> {
        Self::validate_auth_ctx(&request.auth_ctx)?;

        let worker = self.get_or_create(&request).await?;

        let wallet_cards = worker.get_wallet_cards().await?;
        let wallet_cards = wallet_cards
            .into_iter()
            .map(|card| golem_common::serialization::serialize(&card))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| {
                WorkerExecutorError::unknown(format!("Failed to encode wallet card: {e}"))
            })?;

        Ok(GetAgentWalletResponse {
            result: Some(
                golem::workerexecutor::v1::get_agent_wallet_response::Result::Success(
                    GetAgentWalletSuccess { wallet_cards },
                ),
            ),
        })
    }

    pub(super) async fn deliver_card_transfer_internal(
        &self,
        request: DeliverCardTransferRequest,
    ) -> Result<(), WorkerExecutorError> {
        let owned_agent_id = extract_owned_agent_id(
            &request,
            |request| &request.target_agent_id,
            |request| &request.environment_id,
        )?;

        let auth_ctx: AuthCtx = request
            .auth_ctx
            .clone()
            .ok_or(WorkerExecutorError::invalid_request("auth_ctx not found"))?
            .try_into()
            .map_err(|error| {
                WorkerExecutorError::invalid_request(format!("failed converting auth_ctx: {error}"))
            })?;
        auth_ctx
            .authorize_system_only("deliver permission card transfer")
            .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?;

        let transfer_id: uuid::Uuid = request
            .transfer_id
            .ok_or(WorkerExecutorError::invalid_request(
                "transfer_id not found",
            ))?
            .into();
        let source_card_id = CardId(
            request
                .source_card_id
                .ok_or(WorkerExecutorError::invalid_request(
                    "source_card_id not found",
                ))?
                .into(),
        );
        let card: StoredCard = std::panic::catch_unwind(|| desert_rust::deserialize(&request.card))
            .map_err(|_| WorkerExecutorError::invalid_request("invalid card: malformed payload"))?
            .map_err(|error| {
                WorkerExecutorError::invalid_request(format!("invalid card: {error}"))
            })?;

        self.ensure_worker_belongs_to_this_executor(&owned_agent_id)?;
        if Worker::<Ctx>::get_latest_metadata(self, &owned_agent_id)
            .await?
            .is_none()
        {
            let component = self
                .component_service()
                .get_metadata(owned_agent_id.agent_id.component_id, None)
                .await?;
            let parsed_agent_id =
                ParsedAgentId::parse(&owned_agent_id.agent_id.agent_id, &component.metadata)
                    .map_err(|error| {
                        WorkerExecutorError::invalid_request(format!("Invalid agent id: {error}"))
                    })?;
            let target_context =
                agent_monomorphization_context(&component, &owned_agent_id, &parsed_agent_id);
            if !card_matches_agent_recipient(&card, &target_context) {
                return Err(WorkerExecutorError::invalid_request(
                    PERMISSION_CARD_INSTALL_RECIPIENT_MISMATCH,
                ));
            }
        }

        let (worker, _response_lease) =
            if golem_common::model::agent::OwnerKind::is_reserved_instance_name(
                &owned_agent_id.agent_id.agent_id,
            ) {
                Worker::get_exact_existing_suspended_for_response(
                    self,
                    &owned_agent_id,
                    request.principal(),
                )
                .await?
            } else {
                self.get_or_create_pending_with_freshness(
                    &request,
                    InvocationFreshnessDisposition::MayExist,
                )
                .await?
            };
        worker
            .receive_card_transfer(transfer_id, source_card_id, card)
            .await
    }
}
