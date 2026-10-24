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

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    pub async fn begin_function(
        &mut self,
        function_type: &DurableFunctionType,
        invocation_id: Option<DurableInvocationId>,
    ) -> Result<OplogIndex, WorkerExecutorError> {
        self.begin_function_impl(function_type, None, invocation_id)
            .await
            .map(|(index, _)| index)
    }

    pub async fn begin_function_with_span(
        &mut self,
        function_type: &DurableFunctionType,
        span_started: golem_common::model::oplog::SpanStarted,
        invocation_id: Option<DurableInvocationId>,
    ) -> Result<(OplogIndex, golem_common::model::oplog::SpanStarted), WorkerExecutorError> {
        let (index, recorded) = self
            .begin_function_impl(function_type, Some(span_started), invocation_id)
            .await?;
        let recorded = recorded.ok_or_else(|| {
            WorkerExecutorError::unexpected_oplog_entry(
                "durable scope Start carrying span_started",
                format!("durable scope Start at {index} without span metadata"),
            )
        })?;
        Ok((index, recorded))
    }

    /// Checks the effect fence and selects the retry fallback without writing protocol entries.
    async fn begin_function_impl(
        &mut self,
        function_type: &DurableFunctionType,
        span_started: Option<golem_common::model::oplog::SpanStarted>,
        _invocation_id: Option<DurableInvocationId>,
    ) -> Result<(OplogIndex, Option<golem_common::model::oplog::SpanStarted>), WorkerExecutorError>
    {
        if self.state.durability_is_suppressed() {
            let begin_index = self.state.current_oplog_index().await;
            self.state.current_retry_point = begin_index;
            return Ok((begin_index, span_started));
        }

        {
            // No scope opens, so nothing is written before the side effect runs and a fence
            // already latched would only surface at the commit after it - by which time the
            // effect has happened and the shard's new owner, having no record of it, runs it
            // again. Reading the latch costs no storage round trip, so a write whose effect is
            // about to run is refused here instead. It does not close the window where the shard
            // moves *during* the call: that one needs a round trip per call, which is exactly what
            // an idempotent write is declared to avoid.
            if self.state.is_live()
                && matches!(function_type, DurableFunctionType::WriteRemote)
                && let Some(fence) = self.state.oplog.fence()
            {
                return Err(WorkerExecutorError::from(OplogError::Fenced(fence)));
            }

            // Hints are nondeterministic: replay skips them, so they serve neither as retry points
            // nor as the position a call derives its idempotency key from. Live and replay must
            // both return the last non-hint entry, or a repaired call gets a different key.
            let new_retry_point = self
                .state
                .oplog
                .last_added_non_hint_entry()
                .await
                .unwrap_or(self.state.replay_state.last_replayed_non_hint_index());
            self.state.current_retry_point = new_retry_point;

            let begin_index = if self.state.is_live() {
                new_retry_point
            } else {
                self.state.replay_state.last_replayed_non_hint_index()
            };

            Ok((begin_index, span_started))
        }
    }

    pub async fn end_function(
        &mut self,
        function_type: &DurableFunctionType,
        begin_index: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        self.end_function_impl(function_type, begin_index, None)
            .await
    }

    pub(in crate::durable_host) async fn end_function_impl(
        &mut self,
        _function_type: &DurableFunctionType,
        _begin_index: OplogIndex,
        _span_finished: Option<golem_common::model::oplog::SpanFinished>,
    ) -> Result<(), WorkerExecutorError> {
        Ok(())
    }

    /// Claims a transaction protocol Start, or continues live at the replay tail.
    async fn claim_transaction_start(
        &mut self,
        scope_name: &HostFunctionName,
        invocation_id: Option<DurableInvocationId>,
    ) -> Result<Option<(OplogIndex, concurrent::ReplayCallHandle)>, WorkerExecutorError> {
        loop {
            let claim = StartClaim::scope(
                scope_name,
                &DurableFunctionType::WriteRemote,
                self.entity_parent_start_index(),
            )
            .with_invocation_id(invocation_id);
            let expected = claim.expected_description();
            let store_continued_live = self.state.store_continued_live();
            match self
                .state
                .replay_state
                .claim_start_for_store_with_admission(
                    claim.clone(),
                    store_continued_live,
                    |_, _| {},
                    self.entity_tool_operation()
                        .map(|operation| (operation, scope_name.to_string())),
                )
                .await?
            {
                ReplayStartClaimOutcome::Claimed { handle, .. } => {
                    concurrent::reproduce_incomplete_start_refusal(
                        self.entity_tool_operation().as_ref(),
                        &self.state.replay_state,
                        handle.start_idx(),
                    )
                    .await?;
                    return Ok(Some((handle.start_idx(), handle)));
                }
                ReplayStartClaimOutcome::DeletedRegion => {
                    return Err(WorkerExecutorError::unexpected_oplog_entry(
                        expected,
                        "matching Start belongs to a deleted replay region".to_string(),
                    ));
                }
                outcome @ (ReplayStartClaimOutcome::ReplayEnded
                | ReplayStartClaimOutcome::StoreAlreadyLive) => {
                    if self
                        .continue_live_at_replay_tail_with_refusal(
                            matches!(outcome, ReplayStartClaimOutcome::ReplayEnded),
                            expected,
                            Some(concurrent::admission_refusal_before_start(
                                &claim,
                                &scope_name.to_string(),
                            )?),
                        )
                        .await?
                    {
                        return Ok(None);
                    }
                }
            }
        }
    }

    /// Reconstructs the recorded transaction without creating a replacement transaction.
    /// Historical statements remain replayable even when the resident transaction was lost.
    pub async fn begin_transaction_function<Tx>(
        &mut self,
        handler: impl RemoteTransactionHandler<Tx, crate::services::rdbms::RdbmsError>,
    ) -> anyhow::Result<(OplogIndex, Tx)> {
        use concurrent::DurableContext;

        if self.state.durability_is_suppressed() {
            let (_, tx) = handler.create_new().await.map_err(|error| anyhow!(error))?;
            let begin_index = self.state.current_oplog_index().await;
            return Ok((begin_index, tx));
        }

        let scope_name = HostFunctionName::Custom("<scope:transaction>".to_string());
        let invocation_id = self.direct_call_invocation_id(&DurableFunctionType::WriteRemote)?;
        // A transaction begun after the live transition claims first while unclaimed retained
        // `Start`s exist (see `WorkerState::durable_call_is_live`) and begins a fresh live
        // transaction only once replay reports that no scope `Start` remains.
        let claimed_scope = if self.state.durable_call_is_live() {
            None
        } else {
            self.claim_transaction_start(&scope_name, invocation_id)
                .await?
        };

        let Some((scope_start_index, scope_handle)) = claimed_scope else {
            // Commit the scope before opening the external transaction. No statements can
            // execute until the Begin marker is committed and the handle reaches the guest.
            let scope_start = OplogEntry::Start {
                timestamp: Timestamp::now_utc(),
                parent_start_index: self.entity_parent_start_index(),
                function_name: scope_name,
                invocation_id,
                observational_owner: None,
                request: None,
                durable_function_type: DurableFunctionType::WriteRemote,
                span_started: None,
                denial: None,
            };
            let begin_index = self
                .public_state
                .worker()
                .oplog()
                .add(scope_start)
                .await
                .map_err(WorkerExecutorError::from)?;
            self.public_state
                .worker()
                .commit_oplog_and_update_state(CommitLevel::Always)
                .await
                .map_err(WorkerExecutorError::from)?;

            // The transaction scope is now open until commit/rollback; block checkpoints. Opened
            // live, so there is no recorded scope `End` to await on close.
            self.state
                .open_transaction_protocol(begin_index, invocation_id, None);
            self.state.current_retry_point = begin_index;

            let (tx_id, tx) = match handler.create_new().await {
                Ok(tx) => tx,
                Err(error) => {
                    self.finish_transaction_terminal(begin_index,
                        golem_common::model::oplog::payload::types::SerializableRdbmsTransactionTerminal::BeginFailed(Err(error.clone().into())), None).await?;
                    return Err(anyhow!(error));
                }
            };
            self.state
                .oplog
                .add(OplogEntry::begin_remote_transaction(tx_id, begin_index))
                .await?;
            self.public_state
                .worker()
                .commit_oplog_and_update_state(CommitLevel::Always)
                .await?;

            return Ok((begin_index, tx));
        };

        // The transaction scope `Start` is preserved across restarts, so its index is the
        // stable original begin index that keys every transaction marker. Its `End` is folded
        // into the resolver: `claim_scope_start` consumes the `Start`, validates the exact
        // `<scope:transaction>` shape `begin_transaction_function` writes (so a corrupt or
        // interleaved oplog fails here instead of silently driving the recovery logic with the
        // wrong scope), and registers an awaiter the transaction terminal awaits instead of
        // reading the scope `End` positionally.
        let begin_entry = self
            .state
            .replay_state
            .transaction_marker(scope_start_index, |entry| {
                matches!(entry, OplogEntry::BeginRemoteTransaction { .. })
            });
        let Some(begin_entry) = begin_entry else {
            self.state.open_transaction_protocol(
                scope_start_index,
                invocation_id,
                Some(scope_handle),
            );
            if let Some(outcome) = self.replay_transaction_terminal(scope_start_index).await? {
                if let golem_common::model::oplog::payload::types::SerializableRdbmsTransactionTerminal::BeginFailed(Err(error)) = outcome {
                    return Err(anyhow!(crate::services::rdbms::RdbmsError::from(error)));
                }
                return Err(anyhow!("Invalid begin transaction outcome"));
            }
            self.publish_direct_replay_tail(scope_start_index).await?;
            // The previous handle never reached the guest, so it cannot contain statements.
            // Repair only the begin, retaining its scope Start and every sibling call.
            let (tx_id, tx) = match handler.create_new().await {
                Ok(tx) => tx,
                Err(error) => {
                    self.finish_transaction_terminal(scope_start_index,
                        golem_common::model::oplog::payload::types::SerializableRdbmsTransactionTerminal::BeginFailed(Err(error.clone().into())), None).await?;
                    return Err(anyhow!(error));
                }
            };
            self.state
                .oplog
                .add(OplogEntry::begin_remote_transaction(
                    tx_id,
                    scope_start_index,
                ))
                .await?;
            self.public_state
                .worker()
                .commit_oplog_and_update_state(CommitLevel::Always)
                .await?;
            self.state.current_retry_point = scope_start_index;
            return Ok((scope_start_index, tx));
        };
        let original_begin_index = scope_start_index;

        let tx_id = try_match!(
            begin_entry,
            OplogEntry::BeginRemoteTransaction { transaction_id, .. } => transaction_id
        )
        .map_err(|_| WorkerExecutorError::runtime("Unexpected oplog entry"))?;

        let (_, tx) = handler
            .create_replay(&tx_id)
            .await
            .map_err(|error| anyhow!(error))?;
        let result = original_begin_index;
        let scope_replay_handle = Some(scope_handle);

        // Historical statements replay before the terminal resolves external transaction status.
        self.state
            .open_transaction_protocol(result, invocation_id, scope_replay_handle);
        self.state.current_retry_point = original_begin_index;

        Ok((result, tx))
    }

    pub(crate) async fn replay_transaction_terminal(
        &mut self,
        begin_index: OplogIndex,
    ) -> anyhow::Result<
        Option<golem_common::model::oplog::payload::types::SerializableRdbmsTransactionTerminal>,
    > {
        let Some(handle) = self.state.take_transaction_replay_handle(begin_index) else {
            return Ok(None);
        };
        match self
            .state
            .replay_state
            .await_resolution_outcome(handle)
            .await?
        {
            concurrent::ResolutionOutcome::Resolved(concurrent::Resolution::Completed {
                response: Some(response),
                ..
            }) => {
                let response = self
                    .state
                    .oplog
                    .download_payload(response)
                    .await
                    .map_err(anyhow::Error::msg)?;
                let golem_common::model::oplog::HostResponse::GolemRdbmsTransactionTerminal(
                    response,
                ) = response
                else {
                    return Err(anyhow!("Invalid transaction terminal response"));
                };
                self.state.close_transaction_protocol(begin_index)?;
                Ok(Some(response.outcome))
            }
            concurrent::ResolutionOutcome::Incomplete => Ok(None),
            _ => Err(anyhow!("Invalid transaction scope terminal")),
        }
    }

    pub(crate) async fn finish_transaction_terminal(
        &mut self,
        begin_index: OplogIndex,
        outcome: golem_common::model::oplog::payload::types::SerializableRdbmsTransactionTerminal,
        marker: Option<OplogEntry>,
    ) -> anyhow::Result<()> {
        use concurrent::DurableContext;

        if self.state.durability_is_suppressed() {
            return Ok(());
        }
        if !self.is_live() {
            self.publish_direct_replay_tail(begin_index).await?;
        }
        let response = golem_common::model::oplog::HostResponse::GolemRdbmsTransactionTerminal(
            golem_common::model::oplog::HostResponseGolemRdbmsTransactionTerminal { outcome },
        );
        let bytes =
            golem_common::serialization::serialize(&response).map_err(anyhow::Error::msg)?;
        let response = self
            .state
            .oplog
            .upload_raw_payload(bytes)
            .await
            .map_err(anyhow::Error::msg)?
            .into_payload()
            .map_err(anyhow::Error::msg)?;
        if let Some(marker) = marker {
            self.state.oplog.add(marker).await?;
        }
        self.state
            .oplog
            .add(OplogEntry::End {
                timestamp: Timestamp::now_utc(),
                start_index: begin_index,
                response: Some(response),
                forced_commit: true,
                span_finished: None,
                span_attributes: None,
            })
            .await?;
        self.public_state
            .worker()
            .commit_oplog_and_update_state(CommitLevel::Always)
            .await?;
        self.state.close_transaction_protocol(begin_index)?;
        self.maybe_mid_invocation_checkpoint().await;
        Ok(())
    }

    pub async fn pre_commit_transaction_function(
        &mut self,
        begin_index: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        if self.state.durability_is_suppressed() {
            Ok(())
        } else if self.is_live() {
            // There is some logic in the test code that intercepts oplogs adds for _just_ the oplog the is provided to the worker.
            // make sure to write to the local oplog handle, but still commit to the parent for status consistency.
            self.state
                .oplog
                .add(OplogEntry::pre_commit_remote_transaction(begin_index))
                .await?;

            self.public_state
                .worker()
                .commit_oplog_and_update_state(CommitLevel::Always)
                .await?;
            Ok(())
        } else {
            if self
                .state
                .replay_state
                .transaction_marker(begin_index, |entry| {
                    entry.is_pre_commit_remote_transaction(begin_index)
                })
                .is_none()
            {
                return Err(WorkerExecutorError::runtime(
                    "Missing transaction pre-commit marker",
                ));
            }
            Ok(())
        }
    }

    pub async fn pre_rollback_transaction_function(
        &mut self,
        begin_index: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        if self.state.durability_is_suppressed() {
            Ok(())
        } else if self.is_live() {
            // There is some logic in the test code that intercepts oplogs adds for _just_ the oplog the is provided to the worker.
            // make sure to write to the local oplog handle, but still commit to the parent for status consistency.
            self.state
                .oplog
                .add(OplogEntry::pre_rollback_remote_transaction(begin_index))
                .await?;

            self.public_state
                .worker()
                .commit_oplog_and_update_state(CommitLevel::Always)
                .await?;
            Ok(())
        } else {
            if self
                .state
                .replay_state
                .transaction_marker(begin_index, |entry| {
                    entry.is_pre_rollback_remote_transaction(begin_index)
                })
                .is_none()
            {
                return Err(WorkerExecutorError::runtime(
                    "Missing transaction pre-rollback marker",
                ));
            }
            Ok(())
        }
    }
}

impl PrivateDurableWorkerState {
    fn open_transaction_protocol(
        &mut self,
        start_index: OplogIndex,
        invocation_id: Option<DurableInvocationId>,
        replay_end: Option<concurrent::ReplayCallHandle>,
    ) {
        let protocol = TransactionProtocol {
            _invocation_id: invocation_id,
            replay_end,
            _live_call_permit: concurrent::LiveCallPermit::new(self.live_host_calls.clone()),
        };
        assert!(
            self.transaction_protocols
                .insert(start_index, protocol)
                .is_none()
        );
    }

    fn take_transaction_replay_handle(
        &mut self,
        start_index: OplogIndex,
    ) -> Option<concurrent::ReplayCallHandle> {
        self.transaction_protocols
            .get_mut(&start_index)
            .and_then(|protocol| protocol.replay_end.take())
    }

    fn close_transaction_protocol(
        &mut self,
        start_index: OplogIndex,
    ) -> Result<(), WorkerExecutorError> {
        self.transaction_protocols
            .remove(&start_index)
            .ok_or_else(|| {
                WorkerExecutorError::runtime(format!(
                    "Transaction protocol {start_index} is not open"
                ))
            })?;
        Ok(())
    }
}
