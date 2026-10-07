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

//! The update of an instance to a new component revision at the update point: the metadata of the
//! revision, the agent config and wallet cards of the new agent type, and the initial-file rule.
//! The preparation reads and installs; the application to the worker context changes no file.
//! Both the p2 and the p3 replay paths use it.

use super::{DurableWorkerCtx, agent_initial_card_from_component_metadata, replace_wallet_cards};
use crate::services::agent_filesystem::{
    Error as FilesystemError, FilesystemGenerationHandle, update_initial_files,
};
use crate::services::component::ComponentService;
use crate::services::file_loader::FileLoader;
use crate::worker::agent_config::{effective_agent_config, validate_agent_config};
use crate::workerctx::WorkerCtx;
use golem_common::model::OwnedAgentId;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::card::{CardId, StoredCard};
use golem_common::model::component::ComponentRevision;
use golem_common::model::worker::TypedAgentConfigEntry;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::component::Component;
use std::collections::{BTreeMap, HashMap};
use std::fmt::{Display, Formatter};
use std::sync::Arc;

/// Why the update of an instance to a new component revision failed, by its source.
#[derive(Debug)]
pub(crate) enum UpdateStateError {
    /// The metadata of the revision could not be fetched.
    Metadata(WorkerExecutorError),
    /// The revision has no agent type of this name.
    MissingAgentType(String),
    /// The agent config of the new agent type is not valid.
    Config(WorkerExecutorError),
    /// The initial wallet cards of the new agent type could not be built.
    WalletCards(WorkerExecutorError),
    /// The initial-file rule failed.
    InitialFiles(FilesystemError),
}

impl UpdateStateError {
    /// The executor error of the failure.
    pub(crate) fn to_worker_executor_error(&self) -> WorkerExecutorError {
        match self {
            Self::Metadata(error) | Self::Config(error) | Self::WalletCards(error) => error.clone(),
            Self::MissingAgentType(agent_type) => WorkerExecutorError::invalid_request(format!(
                "Agent type {agent_type} not found in updated agent metadata"
            )),
            Self::InitialFiles(error) => {
                crate::worker::start_outcome::reconstruction_startup_error(error)
            }
        }
    }
}

impl Display for UpdateStateError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.to_worker_executor_error(), formatter)
    }
}

/// What the update of an instance reads from its worker context.
pub(crate) struct RevisionUpdateInputs {
    pub(crate) component_service: Arc<dyn ComponentService>,
    pub(crate) file_loader: Arc<FileLoader>,
    pub(crate) filesystem_generation_handle: FilesystemGenerationHandle,
    pub(crate) owned_agent_id: OwnedAgentId,
    pub(crate) agent_id: Option<ParsedAgentId>,
    pub(crate) initial_agent_config: Vec<TypedAgentConfigEntry>,
}

impl RevisionUpdateInputs {
    /// The inputs of `ctx`.
    pub(crate) fn of<Ctx: WorkerCtx>(ctx: &DurableWorkerCtx<Ctx>) -> Self {
        Self {
            component_service: ctx.state.component_service.clone(),
            file_loader: ctx.state.file_loader.clone(),
            filesystem_generation_handle: ctx.filesystem_generation_handle(),
            owned_agent_id: ctx.owned_agent_id.clone(),
            agent_id: ctx.parsed_agent_id(),
            initial_agent_config: ctx.state.initial_agent_config.clone(),
        }
    }
}

type AgentState = (
    HashMap<Vec<String>, golem_common::schema::TypedSchemaValue>,
    BTreeMap<CardId, StoredCard>,
);

/// A prepared update of an instance, which [`apply_revision_update`] applies.
pub(crate) struct RevisionUpdate {
    metadata: Component,
    agent_state: Option<AgentState>,
}

/// Prepares the update of an instance to `new_revision`: it fetches the metadata, builds the agent
/// config and wallet cards of the agent type, and applies the initial-file rule to the agent
/// filesystem.
pub(crate) async fn prepare_revision_update(
    inputs: RevisionUpdateInputs,
    new_revision: ComponentRevision,
) -> Result<RevisionUpdate, UpdateStateError> {
    let RevisionUpdateInputs {
        component_service,
        file_loader,
        filesystem_generation_handle,
        owned_agent_id,
        agent_id,
        initial_agent_config,
    } = inputs;
    let metadata = component_service
        .get_metadata(owned_agent_id.component_id(), Some(new_revision))
        .await
        .map_err(UpdateStateError::Metadata)?;

    let provision_config = agent_id.as_ref().and_then(|agent_id| {
        metadata
            .metadata
            .agent_type_provision_configs()
            .get(&agent_id.agent_type)
            .cloned()
    });

    let agent_state = match &agent_id {
        Some(agent_id) => {
            let agent_type = metadata
                .metadata
                .find_agent_type_by_name_ref(&agent_id.agent_type)
                .ok_or_else(|| {
                    UpdateStateError::MissingAgentType(agent_id.agent_type.to_string())
                })?;
            let updated_agent_config = effective_agent_config(
                initial_agent_config,
                provision_config
                    .as_ref()
                    .map(|config| config.config.clone())
                    .unwrap_or_default(),
            )
            .map_err(UpdateStateError::Config)?;
            validate_agent_config(&updated_agent_config, agent_type)
                .map_err(UpdateStateError::Config)?;
            let initial_card = agent_initial_card_from_component_metadata(&metadata, agent_id)
                .map_err(UpdateStateError::WalletCards)?;
            Some((
                updated_agent_config,
                BTreeMap::from([(initial_card.card_id(), initial_card)]),
            ))
        }
        None => None,
    };

    update_initial_files(
        &filesystem_generation_handle,
        file_loader,
        owned_agent_id.environment_id,
        provision_config
            .as_ref()
            .map(|config| config.files.clone())
            .unwrap_or_default(),
    )
    .map_err(UpdateStateError::InitialFiles)?
    .await
    .map_err(UpdateStateError::InitialFiles)?;

    Ok(RevisionUpdate {
        metadata,
        agent_state,
    })
}

/// Applies a prepared update to the worker context.
pub(crate) fn apply_revision_update<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    update: RevisionUpdate,
) -> Result<(), WorkerExecutorError> {
    ctx.state.component_metadata = update.metadata.clone();
    ctx.executable = crate::workerctx::WorkerCtxExecutable::Component(Box::new(update.metadata));

    if let Some((agent_config, initial_wallet_cards)) = update.agent_state {
        ctx.state.agent_config = agent_config;
        ctx.state.cached_agent_config_retry_policies = None;
        replace_wallet_cards(
            &mut ctx.state.agent_wallet_cards,
            &mut ctx.state.wallet_generation,
            initial_wallet_cards,
        )?;
        ctx.rederive_agent_effective_surface_from_wallet();
    }
    Ok(())
}
