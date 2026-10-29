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

use super::*;

#[derive(Clone)]
struct OplogStartAttributionSource {
    parent_start_index: Option<OplogIndex>,
    observational_owner: Option<OplogIndex>,
    function_name: HostFunctionName,
    request: Option<golem_common::model::oplog::OplogPayload<HostRequest>>,
}

impl OplogStartAttributionSource {
    fn from_entry(entry: &OplogEntry) -> Option<Self> {
        match entry {
            OplogEntry::Start {
                parent_start_index,
                observational_owner,
                function_name,
                request,
                ..
            } => Some(Self {
                parent_start_index: *parent_start_index,
                observational_owner: *observational_owner,
                function_name: function_name.clone(),
                request: request.clone(),
            }),
            _ => None,
        }
    }

    fn parent(&self) -> Option<OplogIndex> {
        self.observational_owner.or(self.parent_start_index)
    }
}

enum AttributionPathNode {
    Inherit(OplogIndex),
    Entity(OplogIndex, PublicEntityInvocation),
    Agent(OplogIndex),
}

pub(super) struct PublicOplogAttributionResolver<'a> {
    oplog_service: Arc<dyn OplogService>,
    owned_agent_id: &'a OwnedAgentId,
    agent_mode: AgentMode,
    starts: HashMap<OplogIndex, Option<OplogStartAttributionSource>>,
    resolved: HashMap<OplogIndex, Option<PublicEntityInvocationContext>>,
}

impl<'a> PublicOplogAttributionResolver<'a> {
    pub(super) fn new(
        oplog_service: Arc<dyn OplogService>,
        owned_agent_id: &'a OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Self {
        Self {
            oplog_service,
            owned_agent_id,
            agent_mode,
            starts: HashMap::new(),
            resolved: HashMap::new(),
        }
    }

    pub(super) fn cache_entry(&mut self, index: OplogIndex, entry: &OplogEntry) {
        self.starts
            .insert(index, OplogStartAttributionSource::from_entry(entry));
    }

    pub(super) async fn attribution_for_entry(
        &mut self,
        index: OplogIndex,
        entry: &OplogEntry,
    ) -> Result<PublicOplogEntryAttribution, String> {
        if let Some(start_index) = entry.entity_parent_start_index() {
            if start_index >= index {
                return Err(format!(
                    "oplog entry {index} has non-causal entity parent Start index {start_index}"
                ));
            }
            let source = self.load_start(start_index).await?.ok_or_else(|| {
                format!(
                    "oplog entry {index} entity parent index {start_index} does not reference a Start"
                )
            })?;
            if source.function_name != HostFunctionName::GolemEntityInvoke {
                return Err(format!(
                    "oplog entry {index} entity parent Start {start_index} is not an entity invocation"
                ));
            }
            return self
                .entity_context_for_start(start_index)
                .await?
                .map(PublicOplogEntryAttribution::entity)
                .ok_or_else(|| {
                    format!(
                        "oplog entry {index} entity parent Start {start_index} has no entity attribution"
                    )
                });
        }

        let owner_start_index = match entry {
            OplogEntry::Start { .. } => Some(index),
            OplogEntry::ToolAttachmentRead { observation, .. } => match observation.observation {
                golem_common::base_model::durable_stream::StreamConsumerObservation::Host => None,
                golem_common::base_model::durable_stream::StreamConsumerObservation::Guest {
                    parent_start_index,
                } => parent_start_index,
            },
            OplogEntry::End { start_index, .. }
            | OplogEntry::Cancelled { start_index, .. }
            | OplogEntry::CompletionDiscarded { start_index, .. }
            | OplogEntry::CompletionDelivered { start_index, .. } => Some(*start_index),
            OplogEntry::HostStreamFrame {
                parent_start_index, ..
            }
            | OplogEntry::Log {
                parent_start_index: Some(parent_start_index),
                ..
            } => Some(*parent_start_index),
            OplogEntry::Error {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::Jump {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CreateResource {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::DropResource {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::SetRetryPolicy {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::RemoveRetryPolicy {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardEventQueued {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardInstalled {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardInstallFailed {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardRevoked {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardExpired {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardDerived {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardTransferStarted {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardTransferred {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardRevokedCascade {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::CardTransferConfirmed {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::StreamRegistered {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::StreamItems {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::StreamEnd {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::StreamCancel {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::StreamSession {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::ToolAttachmentControl {
                entity_parent_start_index,
                ..
            }
            | OplogEntry::SpanClosed {
                entity_parent_start_index,
                ..
            } => *entity_parent_start_index,
            OplogEntry::BeginRemoteTransaction {
                original_begin_index: begin_index,
                ..
            } => Some(*begin_index),
            OplogEntry::PreCommitRemoteTransaction { begin_index, .. }
            | OplogEntry::PreRollbackRemoteTransaction { begin_index, .. }
            | OplogEntry::CommittedRemoteTransaction { begin_index, .. }
            | OplogEntry::RolledBackRemoteTransaction { begin_index, .. } => Some(*begin_index),
            OplogEntry::Create { .. }
            | OplogEntry::AgentInvocationStarted { .. }
            | OplogEntry::AgentInvocationFinished { .. }
            | OplogEntry::Suspend { .. }
            | OplogEntry::RecoverySucceeded { .. }
            | OplogEntry::Interrupted { .. }
            | OplogEntry::Exited { .. }
            | OplogEntry::PendingAgentInvocation { .. }
            | OplogEntry::PendingUpdate { .. }
            | OplogEntry::SuccessfulUpdate { .. }
            | OplogEntry::FailedUpdate { .. }
            | OplogEntry::GrowMemory { .. }
            | OplogEntry::Log {
                parent_start_index: None,
                ..
            }
            | OplogEntry::Restart { .. }
            | OplogEntry::Resumed { .. }
            | OplogEntry::ActivatePlugin { .. }
            | OplogEntry::DeactivatePlugin { .. }
            | OplogEntry::Revert { .. }
            | OplogEntry::CancelPendingInvocation { .. }
            | OplogEntry::Snapshot { .. }
            | OplogEntry::SnapshotConfirmed { .. }
            | OplogEntry::OplogProcessorCheckpoint { .. } => None,
        };

        match owner_start_index {
            Some(start_index) => self
                .entity_context_for_start(start_index)
                .await
                .map(|context| {
                    context.map_or_else(
                        PublicOplogEntryAttribution::agent,
                        PublicOplogEntryAttribution::entity,
                    )
                }),
            None => Ok(PublicOplogEntryAttribution::agent()),
        }
    }

    async fn entity_context_for_start(
        &mut self,
        start_index: OplogIndex,
    ) -> Result<Option<PublicEntityInvocationContext>, String> {
        let mut path = Vec::new();
        let mut visited = HashSet::new();
        let mut current = start_index;
        let mut context = loop {
            if let Some(resolved) = self.resolved.get(&current) {
                break resolved.clone();
            }
            if !visited.insert(current) {
                return Err(format!("cyclic oplog Start attribution at index {current}"));
            }

            let Some(source) = self.load_start(current).await? else {
                break None;
            };
            if source.function_name == HostFunctionName::GolemToolInvocationRejected {
                path.push(AttributionPathNode::Agent(current));
                break None;
            }
            if source.function_name == HostFunctionName::GolemEntityInvoke {
                let invocation = self.load_public_entity_invocation(current, &source).await?;
                let parent = source.parent();
                path.push(AttributionPathNode::Entity(current, invocation));
                match parent {
                    Some(parent) => current = parent,
                    None => break None,
                }
            } else {
                let parent = source.parent();
                path.push(AttributionPathNode::Inherit(current));
                match parent {
                    Some(parent) => current = parent,
                    None => break None,
                }
            }
        };

        for node in path.into_iter().rev() {
            let index = match node {
                AttributionPathNode::Inherit(index) => index,
                AttributionPathNode::Agent(index) => {
                    context = None;
                    index
                }
                AttributionPathNode::Entity(index, invocation) => {
                    let ancestors = context
                        .as_ref()
                        .map(|parent| {
                            let mut ancestors = parent.ancestors.clone();
                            ancestors.push(parent.invocation.clone());
                            ancestors
                        })
                        .unwrap_or_default();
                    context = Some(PublicEntityInvocationContext {
                        invocation,
                        ancestors,
                    });
                    index
                }
            };
            self.resolved.insert(index, context.clone());
        }

        Ok(context)
    }

    async fn load_start(
        &mut self,
        index: OplogIndex,
    ) -> Result<Option<OplogStartAttributionSource>, String> {
        if let Some(start) = self.starts.get(&index) {
            return Ok(start.clone());
        }

        let entry = self
            .oplog_service
            .read_exact(self.owned_agent_id, self.agent_mode, index, 1)
            .await
            .map_err(|error| error.to_string())?
            .remove(&index);
        let start = entry
            .as_ref()
            .and_then(OplogStartAttributionSource::from_entry);
        self.starts.insert(index, start.clone());
        Ok(start)
    }

    async fn load_public_entity_invocation(
        &self,
        start_index: OplogIndex,
        source: &OplogStartAttributionSource,
    ) -> Result<PublicEntityInvocation, String> {
        let request_payload = source
            .request
            .clone()
            .ok_or_else(|| format!("entity invocation Start {start_index} has no request"))?;
        let request: HostRequest = self
            .oplog_service
            .download_payload(self.owned_agent_id, self.agent_mode, request_payload)
            .await?;
        let request = match request {
            HostRequest::EntityInvocation(request) => request,
            actual => {
                return Err(format!(
                    "entity invocation Start {start_index} has unexpected request {actual:?}"
                ));
            }
        };
        let metadata = desert_rust::deserialize::<EntityInvocationRequest>(&request.metadata)
            .map_err(|error| {
                format!("failed to decode entity invocation Start {start_index}: {error}")
            })?;

        Ok(public_entity_invocation(start_index, metadata))
    }
}
