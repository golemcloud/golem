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

impl PrimaryInvocationBody {
    pub(crate) fn invocation(&self) -> Option<&crate::worker::owner_lane::OwnerInvocationId> {
        self.permit.as_ref().map(OwnerInvocationPermit::invocation)
    }

    pub(crate) fn complete(mut self) {
        if let Some(permit) = self.permit.take() {
            permit.complete();
        }
    }
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    pub fn set_entity_invocation_scope(
        &mut self,
        scope: Option<EntityInvocationScope>,
    ) -> Result<(), WorkerExecutorError> {
        match (&self.runtime, &self.entity_invocation_scope, &scope) {
            (OwnerRuntime::Agent, _, Some(_)) => Err(WorkerExecutorError::runtime(
                "Cannot install an entity invocation scope in the primary Store",
            )),
            (OwnerRuntime::Agent, _, None) => Ok(()),
            (OwnerRuntime::Entity(_), Some(_), Some(_)) => Err(WorkerExecutorError::runtime(
                "Entity invocation scope is already installed",
            )),
            (OwnerRuntime::Entity(_), None, None) => Err(WorkerExecutorError::runtime(
                "Entity invocation scope is not installed",
            )),
            _ => {
                if let Some(scope) = &scope {
                    if let Some(started) = scope.span_started() {
                        let span = InvocationContextSpan::local()
                            .with_span_id(started.span_id.clone())
                            .with_start(started.started_at)
                            .parent(started.parent_span_id.as_ref().map(|span_id| {
                                InvocationContextSpan::external_parent(span_id.clone())
                            }))
                            .with_attributes(started.attributes.0.clone())
                            .build();
                        let stack = InvocationContextStack::new(
                            started.trace_id.clone(),
                            span,
                            started.trace_states.clone(),
                        )
                        .limit_depth(self.state.config.limits.max_invocation_context_stack_depth);
                        let (context, current_span_id) = InvocationContext::from_stack(stack)
                            .map_err(WorkerExecutorError::runtime)?;
                        self.state.invocation_context.switch_to(context);
                        self.state.current_span_id = current_span_id;
                    }
                    self.state
                        .set_current_idempotency_key(scope.idempotency_key().clone());
                    self.state.entity_logical_key_position =
                        scope.logical_key_positions().then_some(OplogIndex::INITIAL);
                } else {
                    self.state.current_idempotency_key = None;
                    self.state.entity_logical_key_position = None;
                }
                self.state.inherited_invocation_scope = scope
                    .as_ref()
                    .and_then(EntityInvocationScope::invocation_scope_card)
                    .cloned()
                    .map(InheritedInvocationScope::unchecked);
                if let Some(scope) = &scope {
                    let roots = scope
                        .invocation_scope_card()
                        .map(|card| card.root_card_ids.as_slice())
                        .unwrap_or_default();
                    self.state.scope_interest = Some(
                        self.state
                            .owner_wallet
                            .interest
                            .contribute(Some(scope.invocation_id().clone()), roots),
                    );
                }
                // Clearing execution state precedes retained Store settlement. The retained
                // Store keeps its roots registered until settlement or actual invalidation.
                // A changed scope is not represented by a worker-status generation. Force the next
                // live authorization through the synchronized path, which checks inherited roots.
                self.state.authority_initialized = false;
                self.rederive_agent_effective_surface_from_wallet();
                self.entity_invocation_scope = scope;
                Ok(())
            }
        }
    }

    pub fn entity_invocation_scope(&self) -> Option<&EntityInvocationScope> {
        self.entity_invocation_scope.as_ref()
    }

    pub(crate) fn set_entity_tool_operation(
        &mut self,
        operation: tool::operation::OwnerToolOperation,
    ) -> Result<(), WorkerExecutorError> {
        if !matches!(
            self.runtime,
            OwnerRuntime::Entity(AgentEntity::Tool(_) | AgentEntity::ToolMiddleware(_))
        ) {
            return Err(WorkerExecutorError::runtime(
                "Tool operation can only be installed in a tool entity Store",
            ));
        }
        if self.entity_tool_operation.replace(operation).is_some() {
            return Err(WorkerExecutorError::runtime(
                "Tool operation is already installed in the entity Store",
            ));
        }
        Ok(())
    }

    pub(in crate::durable_host) fn entity_tool_operation(
        &self,
    ) -> Option<tool::operation::OwnerToolOperation> {
        self.entity_tool_operation.clone()
    }

    pub(crate) fn set_invocation_principal(&mut self, principal: Option<Principal>) {
        self.invocation_principal = principal;
    }

    pub(crate) fn invocation_principal(&self) -> Principal {
        self.invocation_principal.clone().unwrap_or_else(|| {
            Principal::Agent(AgentPrincipal {
                agent_id: self.owned_agent_id.agent_id.clone(),
            })
        })
    }

    pub(crate) fn set_entity_cancellation(
        &mut self,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<(), WorkerExecutorError> {
        if !matches!(self.runtime, OwnerRuntime::Entity(_)) {
            return Err(WorkerExecutorError::runtime(
                "Cannot install entity cancellation in the primary Store",
            ));
        }
        if self.entity_cancellation.is_some() {
            return Err(WorkerExecutorError::runtime(
                "Entity cancellation is already installed",
            ));
        }
        self.entity_cancellation = Some(cancellation);
        Ok(())
    }

    pub(in crate::durable_host) fn entity_cancellation(
        &self,
    ) -> Option<tokio_util::sync::CancellationToken> {
        if ENTITY_CANCELLATION_MASKED.try_with(|()| ()).is_ok() {
            None
        } else {
            self.entity_cancellation.clone()
        }
    }

    /// The index of the record that numbers the guest calls of the current invocation: the entity
    /// invocation `Start` of an entity Store, or the `AgentInvocationStarted` of the primary Store.
    pub(crate) fn invocation_anchor(&self) -> Option<OplogIndex> {
        self.entity_parent_start_index().or(self
            .primary_invocation_start_index
            .filter(|index| *index != OplogIndex::NONE))
    }

    pub(in crate::durable_host) fn guest_read_order(&self) -> Option<replay_state::GuestReadOrder> {
        self.invocation_anchor()
            .map(|anchor| replay_state::GuestReadOrder { anchor })
    }

    /// Reserves the next ordinal in guest call order for a direct Store-holding durable call.
    pub(crate) fn direct_call_invocation_id(
        &mut self,
        _function_type: &DurableFunctionType,
    ) -> Result<Option<DurableInvocationId>, WorkerExecutorError> {
        if self.state.snapshotting_mode {
            return Ok(None);
        }
        let observed_by_custom_invocation = wasmtime::component::current_guest_task_context::<
            durability::CustomInvocationContext,
        >()
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
        .as_deref()
        .and_then(durability::CustomInvocationContext::current)
        .is_some();
        if observed_by_custom_invocation {
            return Ok(None);
        }
        let Some(anchor) = self.invocation_anchor() else {
            return Ok(None);
        };
        durability::reserve_invocation_id(self, None, anchor).map(Some)
    }

    pub(in crate::durable_host) fn entity_parent_start_index(&self) -> Option<OplogIndex> {
        self.entity_invocation_scope
            .as_ref()
            .map(|scope| scope.invocation_id().start_index())
    }

    pub(crate) fn owner_invocation_id(&self) -> Result<OwnerInvocationId, WorkerExecutorError> {
        match &self.runtime {
            OwnerRuntime::Entity(_) => self
                .entity_invocation_scope
                .as_ref()
                .map(|scope| OwnerInvocationId::Entity(scope.invocation_id().clone()))
                .ok_or_else(|| {
                    WorkerExecutorError::runtime(
                        "Entity Store has no active entity invocation scope",
                    )
                }),
            OwnerRuntime::Agent => self
                .primary_invocation_start_index
                .filter(|index| *index != OplogIndex::NONE)
                .map(OwnerInvocationId::Agent)
                .ok_or_else(|| {
                    WorkerExecutorError::runtime("Primary Store has no active invocation Start")
                }),
        }
    }

    pub(crate) async fn enter_primary_invocation_body(
        &self,
    ) -> Result<Option<PrimaryInvocationBody>, WorkerExecutorError> {
        if self.runtime != OwnerRuntime::Agent {
            return Ok(None);
        }
        let start_index = if self.state.snapshotting_mode {
            self.state.oplog.current_oplog_index().await
        } else {
            match self.owner_invocation_id()? {
                OwnerInvocationId::Agent(index) => index,
                OwnerInvocationId::Entity(_) => unreachable!(),
            }
        };
        let ticket = self
            .owner_execution
            .lane()
            .enter_primary(start_index)
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        let permit = ticket
            .acquire()
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        Ok(Some(PrimaryInvocationBody {
            permit: Some(permit),
        }))
    }
}
