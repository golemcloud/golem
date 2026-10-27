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

use super::super::*;

/// The ID of the runtime card that the durable call at `oplog_index` derives.
pub(crate) fn derived_card_id(
    owned_agent_id: &OwnedAgentId,
    invocation_key: &IdempotencyKey,
    oplog_index: OplogIndex,
) -> CardId {
    CardId(derive_permission_uuid(
        DERIVED_CARD_ID_CONTEXT,
        owned_agent_id,
        invocation_key,
        oplog_index,
    ))
}

/// The ID of the card transfer that the durable call at `oplog_index` performs.
pub(crate) fn transfer_id(
    owned_agent_id: &OwnedAgentId,
    invocation_key: &IdempotencyKey,
    oplog_index: OplogIndex,
) -> Uuid {
    derive_permission_uuid(
        TRANSFER_ID_CONTEXT,
        owned_agent_id,
        invocation_key,
        oplog_index,
    )
}

/// The ID of the child card that the durable call at `oplog_index` installs.
pub(crate) fn installed_child_card_id(
    owned_agent_id: &OwnedAgentId,
    invocation_key: &IdempotencyKey,
    oplog_index: OplogIndex,
) -> CardId {
    CardId(derive_permission_uuid(
        INSTALLED_CHILD_CARD_ID_CONTEXT,
        owned_agent_id,
        invocation_key,
        oplog_index,
    ))
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    pub(crate) fn derive_scope_card_id(
        &self,
        invocation_key: &IdempotencyKey,
        ordinal: u64,
    ) -> CardId {
        CardId(derive_permission_uuid_for_sequence(
            SCOPE_CARD_ID_CONTEXT,
            &self.owned_agent_id,
            invocation_key,
            ordinal,
        ))
    }

    pub fn agent_effective_surface(&self) -> golem_common::model::card::EffectiveSurface {
        self.state.agent_effective_surface.clone()
    }

    pub fn agent_auth_ctx(&self) -> AuthCtx {
        let context = match &self.state.owner_context {
            ResolvedOwnerContext::Agent(agent_id) => agent_monomorphization_context(
                self.authority_component_metadata(),
                &self.owned_agent_id,
                agent_id,
            ),
            ResolvedOwnerContext::ComponentWorker | ResolvedOwnerContext::ComponentBaseline => {
                component_baseline_monomorphization_context(
                    self.authority_component_metadata(),
                    &self.owned_agent_id,
                )
            }
        };
        let delegation_surface = golem_common::model::card::agent_delegation_surface_from_wallet(
            &context,
            self.authority_wallet_cards(),
        );

        AuthCtx::agent_with_permission_surfaces(
            self.created_by(),
            self.created_by_email().clone(),
            self.agent_effective_surface(),
            delegation_surface,
        )
    }

    fn authority_wallet_cards(&self) -> impl Iterator<Item = &StoredCard> {
        let cards = if self.state.is_live() {
            &self.state.authority_wallet.cards
        } else {
            &self.state.agent_wallet_cards
        };
        cards
            .iter()
            .filter(|(card_id, _)| !self.state.owner_revoked_card_ids.contains(card_id))
            .map(|(_, card)| card.as_ref())
    }

    pub fn authority_component_metadata(&self) -> &Component {
        if self.state.is_live() {
            &self.state.authority_wallet.metadata
        } else {
            self.owner_component_metadata()
        }
    }

    pub(crate) fn agent_wallet_cards_snapshot(&self) -> Vec<StoredCard> {
        if self.state.is_live() {
            self.state
                .owner_wallet
                .snapshot()
                .cards
                .values()
                .map(|card| card.as_ref().clone())
                .collect()
        } else {
            self.state
                .agent_wallet_cards
                .values()
                .map(|card| card.as_ref().clone())
                .collect()
        }
    }

    pub(crate) fn wallet_id_hash(&self) -> [u8; 32] {
        self.state.wallet_id_hash
    }

    pub(crate) fn wallet_generation(&self) -> u64 {
        if self.state.is_live() {
            self.state.owner_wallet.snapshot().generation
        } else {
            self.state.wallet_generation
        }
    }

    pub(crate) async fn active_agent_wallet_cards_snapshot(
        &mut self,
    ) -> Result<Vec<StoredCard>, WorkerExecutorError> {
        let _boundary_guard = self
            .lock_synchronized_card_event_boundary_with_authority()
            .await?;
        if self.state.is_replay() {
            return Ok(self.agent_wallet_cards_snapshot());
        }

        self.public_state
            .worker()
            .commit_oplog_and_update_state(CommitLevel::Always)
            .await?;
        self.check_post_replay_wallet_liveness().await?;
        self.drain_card_events_at_boundary().await?;
        let pending_revoked_cards = self
            .pending_card_events_at_boundary()
            .await
            .into_iter()
            .filter_map(|pending_event| match pending_event.event {
                QueuedCardEvent::Revoke(event) => Some(event.card_id),
                QueuedCardEvent::Install(_)
                | QueuedCardEvent::TransferStarted(_)
                | QueuedCardEvent::TransferReceived(_) => None,
            })
            .collect::<HashSet<_>>();
        let wallet = self
            .agent_wallet_cards_snapshot()
            .into_iter()
            .filter(|card| !pending_revoked_cards.contains(&card.card_id()))
            .collect::<Vec<_>>();
        if wallet.is_empty() {
            return Ok(wallet);
        }

        let card_states = self
            .state
            .card_service
            .check_cards(wallet.iter().map(StoredCard::card_id).collect())
            .await?;

        Ok(wallet
            .into_iter()
            .filter(|card| matches!(card_states.get(&card.card_id()), Some(CardState::Live(_))))
            .collect())
    }

    /// Synchronizes the invocation-pinned wallet at the authorization linearization point.
    /// Replay applies only recorded events; live execution also drains queued registry events.
    pub(crate) async fn synchronize_agent_wallet_at_boundary(
        &mut self,
    ) -> Result<(), WorkerExecutorError> {
        if self.state.is_live() {
            self.capture_live_agent_authority_at_boundary(&mut |_| ())
                .await?;
        } else {
            let _boundary_guard = self.lock_synchronized_card_event_boundary().await?;
        }
        Ok(())
    }

    pub(crate) async fn capture_agent_auth_ctx_at_boundary(
        &mut self,
    ) -> Result<Option<AuthCtx>, WorkerExecutorError> {
        self.capture_live_agent_authority_at_boundary(&mut |ctx| ctx.agent_auth_ctx())
            .await
    }

    pub(crate) async fn capture_live_agent_authority_at_boundary<T>(
        &mut self,
        capture: &mut impl FnMut(&mut Self) -> T,
    ) -> Result<Option<T>, WorkerExecutorError> {
        self.process_pending_replay_events().await?;
        if !self.state.is_live() {
            return Ok(None);
        }

        let published_generation = self
            .state
            .published_authority_generation
            .load(Ordering::Acquire);
        let now = Utc::now();
        if authority_snapshot_is_current_at(
            self.state.authority_initialized,
            self.state.card_interest_index.authority_is_open(),
            self.state.processed_authority_generation,
            published_generation,
            self.state.next_authority_expiration,
            now,
        ) {
            let captured = capture(self);
            if self.authority_snapshot_is_stable(published_generation) {
                crate::metrics::wasm::record_agent_permission_authority_fast_path();
                return Ok(Some(captured));
            }
        }

        let started = Instant::now();
        loop {
            let boundary_guard = self
                .lock_synchronized_card_event_boundary_with_authority()
                .await?;
            let generation = self
                .state
                .published_authority_generation
                .load(Ordering::Acquire);
            let captured = self.state.is_live().then(|| capture(self));
            self.refresh_authority_expiration_deadline();
            if self.authority_snapshot_is_stable(generation) {
                self.adopt_authority_generation(generation);
                crate::metrics::wasm::record_agent_permission_authority_slow_path(
                    started.elapsed(),
                );
                return Ok(captured);
            }
            drop(boundary_guard);
        }
    }

    pub(in crate::durable_host) fn authority_snapshot_is_stable(&self, generation: u64) -> bool {
        self.state.replay_applications.is_empty()
            && self.state.owner_wallet.validate_current().is_ok()
            && Arc::ptr_eq(
                &self.state.authority_wallet,
                &self.state.owner_wallet.snapshot(),
            )
            && authority_snapshot_is_current_at(
                true,
                self.state.card_interest_index.authority_is_open(),
                generation,
                self.state
                    .published_authority_generation
                    .load(Ordering::Acquire),
                self.state.next_authority_expiration,
                Utc::now(),
            )
    }

    pub(in crate::durable_host) fn refresh_authority_expiration_deadline(&mut self) {
        let cards = if self.state.is_live() {
            &self.state.authority_wallet.cards
        } else {
            &self.state.agent_wallet_cards
        };
        self.state.next_authority_expiration = self
            .state
            .invocation_scope_root_cards
            .values()
            .chain(cards.values().map(Arc::as_ref))
            .chain(
                self.state
                    .inherited_invocation_scope
                    .as_ref()
                    .and_then(InheritedInvocationScope::live_roots)
                    .into_iter()
                    .flat_map(BTreeMap::values),
            )
            .filter_map(StoredCard::expires_at)
            .min();
    }

    pub(in crate::durable_host) fn adopt_authority_generation(&mut self, generation: u64) {
        self.refresh_authority_expiration_deadline();
        self.state.processed_authority_generation = generation;
        self.state.authority_initialized = true;
    }

    pub(crate) async fn authorize_live_permission(
        &mut self,
        target: &PermissionTarget,
    ) -> Result<Result<LiveAuthorizationPermit, AuthorizationError>, WorkerExecutorError> {
        self.authorize_live_permissions(std::slice::from_ref(target))
            .await
    }

    pub(crate) async fn authorize_live_permissions(
        &mut self,
        targets: &[PermissionTarget],
    ) -> Result<Result<LiveAuthorizationPermit, AuthorizationError>, WorkerExecutorError> {
        self.process_pending_replay_events().await?;
        if self.state.durability_is_suppressed() && !self.state.is_live() {
            // In snapshotting mode a durable call resolves as live and records nothing, also while
            // the Store still replays, as the load of a snapshot during recovery does. Such a call
            // is not checked: no recorded decision exists for it, and replay must not decide one.
            return Ok(Ok(LiveAuthorizationPermit { _private: () }));
        }
        assert_live_authorization(self.state.is_live());

        if self.operator_authorizes_current_invocation() {
            record_permission_decisions(targets, true);
            return Ok(Ok(LiveAuthorizationPermit { _private: () }));
        }

        let published_generation = self
            .state
            .published_authority_generation
            .load(Ordering::Acquire);
        let now = Utc::now();
        if authority_snapshot_is_current_at(
            self.state.authority_initialized,
            self.state.card_interest_index.authority_is_open(),
            self.state.processed_authority_generation,
            published_generation,
            self.state.next_authority_expiration,
            now,
        ) {
            let result = authorize_effective_surface(&self.state.agent_effective_surface, targets);
            if self.authority_snapshot_is_stable(published_generation) {
                crate::metrics::wasm::record_agent_permission_authority_fast_path();
                record_permission_decisions(targets, result.is_ok());
                return Ok(result.map(|()| LiveAuthorizationPermit { _private: () }));
            }
        }

        let started = Instant::now();
        loop {
            let boundary_guard = self
                .lock_synchronized_card_event_boundary_with_authority()
                .await?;
            let generation = self
                .state
                .published_authority_generation
                .load(Ordering::Acquire);
            let result = authorize_effective_surface(&self.state.agent_effective_surface, targets);
            // The previous cached deadline may be due even though synchronization just removed
            // every expired card. Refresh it before validating the synchronized snapshot.
            self.refresh_authority_expiration_deadline();
            if self.authority_snapshot_is_stable(generation) {
                self.adopt_authority_generation(generation);
                crate::metrics::wasm::record_agent_permission_authority_slow_path(
                    started.elapsed(),
                );
                record_permission_decisions(targets, result.is_ok());
                return Ok(result.map(|()| LiveAuthorizationPermit { _private: () }));
            }
            drop(boundary_guard);
        }
    }

    pub(crate) async fn filter_live_permissions(
        &mut self,
        targets: &[PermissionTarget],
    ) -> Result<Vec<bool>, WorkerExecutorError> {
        self.process_pending_replay_events().await?;
        assert!(self.state.is_live());
        if self.operator_authorizes_current_invocation() {
            record_permission_decisions(targets, true);
            return Ok(vec![true; targets.len()]);
        }
        self.capture_live_agent_authority_at_boundary(&mut |ctx| {
            targets
                .iter()
                .map(|target| {
                    ctx.state
                        .agent_effective_surface
                        .authorize(target)
                        .unwrap_or(false)
                })
                .collect()
        })
        .await?
        .ok_or_else(|| WorkerExecutorError::runtime("authorization left live execution"))
    }

    pub(crate) async fn with_agent_authority_at_boundary<T>(
        &mut self,
        capture: impl FnOnce(&mut Self) -> T,
    ) -> Result<T, WorkerExecutorError> {
        let _boundary_guard = self
            .lock_synchronized_card_event_boundary_with_authority()
            .await?;
        let result = capture(self);
        self.state.authority_initialized = false;
        Ok(result)
    }

    pub(crate) async fn try_agent_auth_ctx_at_boundary(
        &mut self,
    ) -> Result<Option<AuthCtx>, WorkerExecutorError> {
        let Some(_boundary_guard) = self
            .try_lock_synchronized_card_event_boundary_with_authority()
            .await?
        else {
            return Ok(None);
        };
        let generation = self
            .state
            .published_authority_generation
            .load(Ordering::Acquire);
        self.refresh_authority_expiration_deadline();
        if !self.authority_snapshot_is_stable(generation) {
            return Ok(None);
        }
        self.adopt_authority_generation(generation);
        Ok(Some(self.agent_auth_ctx()))
    }

    pub(in crate::durable_host) async fn lock_synchronized_card_event_boundary(
        &mut self,
    ) -> Result<Arc<CardEventBoundary>, WorkerExecutorError> {
        self.lock_synchronized_card_event_boundary_inner(false, true)
            .await?
            .ok_or_else(|| WorkerExecutorError::runtime("unrestricted card boundary was closed"))
    }

    async fn lock_synchronized_card_event_boundary_with_authority(
        &mut self,
    ) -> Result<Arc<CardEventBoundary>, WorkerExecutorError> {
        self.lock_synchronized_card_event_boundary_inner(true, true)
            .await?
            .ok_or_else(|| WorkerExecutorError::runtime("card authority boundary did not reopen"))
    }

    async fn try_lock_synchronized_card_event_boundary_with_authority(
        &mut self,
    ) -> Result<Option<Arc<CardEventBoundary>>, WorkerExecutorError> {
        self.lock_synchronized_card_event_boundary_inner(true, false)
            .await
    }

    async fn lock_synchronized_card_event_boundary_inner(
        &mut self,
        requires_agent_authority: bool,
        wait_for_authority: bool,
    ) -> Result<Option<Arc<CardEventBoundary>>, WorkerExecutorError> {
        loop {
            self.process_pending_replay_events().await?;
            if !self.state.is_live() {
                return Ok(Some(Arc::new(CardEventBoundary::Historical)));
            }
            let owner = self.state.owner_wallet.clone();
            let worker = self.public_state.worker().clone();
            let inputs = self.authority_scope_inputs();
            let Some(observation) = owner
                .synchronize(worker, inputs, requires_agent_authority, wait_for_authority)
                .await?
            else {
                return Ok(None);
            };
            let generation = observation.generation;
            if !self.apply_owner_authority_snapshot(observation)? {
                continue;
            }
            let boundary = owner.lock_boundary().await?;
            if let Some(application) = owner.pending_replay_finalization() {
                drop(boundary);
                application.result.clone().await?;
                continue;
            }
            if generation
                != self
                    .state
                    .published_authority_generation
                    .load(Ordering::Acquire)
                || !Arc::ptr_eq(&self.state.authority_wallet, &owner.snapshot())
                || (requires_agent_authority && !self.state.card_interest_index.authority_is_open())
            {
                drop(boundary);
                continue;
            }
            self.state.card_boundary = Arc::downgrade(&boundary);
            return Ok(Some(boundary));
        }
    }

    pub(in crate::durable_host) fn authority_scope_inputs(&self) -> AuthorityScopeInputs {
        AuthorityScopeInputs {
            scope: self.current_invocation_scope_card(),
            roots: self
                .state
                .invocation_scope_root_cards
                .values()
                .chain(
                    self.state
                        .inherited_invocation_scope
                        .as_ref()
                        .and_then(InheritedInvocationScope::live_roots)
                        .into_iter()
                        .flat_map(BTreeMap::values),
                )
                .cloned()
                .collect(),
            unchecked_root_ids: self
                .state
                .inherited_invocation_scope
                .as_ref()
                .and_then(InheritedInvocationScope::unchecked_root_ids),
            parent: self.entity_parent_start_index(),
        }
    }

    pub(in crate::durable_host) fn apply_owner_authority_snapshot(
        &mut self,
        observation: OwnerAuthoritySnapshot,
    ) -> Result<bool, WorkerExecutorError> {
        self.state.owner_wallet.validate_current()?;
        if observation.scope != self.current_invocation_scope_card()
            || observation.generation
                != self
                    .state
                    .published_authority_generation
                    .load(Ordering::Acquire)
            || !Arc::ptr_eq(&observation.wallet, &self.state.owner_wallet.snapshot())
        {
            return Ok(false);
        }
        self.state.authority_wallet = observation.wallet;
        self.apply_owner_authority_synchronization(observation.revoked, observation.root_states)?;
        self.adopt_authority_generation(observation.generation);
        self.state.authority_initialized = self.state.card_interest_index.authority_is_open();
        Ok(observation.generation
            == self
                .state
                .published_authority_generation
                .load(Ordering::Acquire)
            && Arc::ptr_eq(
                &self.state.authority_wallet,
                &self.state.owner_wallet.snapshot(),
            ))
    }

    pub(crate) async fn commit_wallet_entry(
        &mut self,
        entry: OplogEntry,
    ) -> Result<(), WorkerExecutorError> {
        let owner = self.state.owner_wallet.clone();
        let boundary = match self.state.card_boundary.upgrade() {
            Some(boundary) if matches!(boundary.as_ref(), CardEventBoundary::Live { .. }) => {
                boundary
            }
            _ => owner.lock_boundary().await?,
        };
        owner
            .commit(&self.public_state.worker(), boundary, entry)
            .await?;
        self.state.authority_initialized = false;
        self.state.authority_wallet = owner.snapshot();
        self.rederive_agent_effective_surface_from_wallet();
        Ok(())
    }

    pub(in crate::durable_host) fn rederive_agent_effective_surface_from_wallet(&mut self) {
        let context = match &self.state.owner_context {
            ResolvedOwnerContext::Agent(agent_id) => agent_monomorphization_context(
                self.authority_component_metadata(),
                &self.owned_agent_id,
                agent_id,
            ),
            ResolvedOwnerContext::ComponentWorker | ResolvedOwnerContext::ComponentBaseline => {
                component_baseline_monomorphization_context(
                    self.authority_component_metadata(),
                    &self.owned_agent_id,
                )
            }
        };
        let surface = {
            let wallet = self.authority_wallet_cards();
            match &self.runtime {
                OwnerRuntime::Agent => {
                    golem_common::model::card::agent_effective_surface_from_wallet_and_scope(
                        &context,
                        wallet,
                        self.state.invocation_scope_card.as_ref(),
                    )
                }
                OwnerRuntime::Entity(_) => match &self.state.inherited_invocation_scope {
                    None => golem_common::model::card::agent_effective_surface_from_wallet(
                        &context, wallet,
                    ),
                    Some(scope) => {
                        golem_common::model::card::agent_effective_surface_from_wallet_and_scope(
                            &context,
                            wallet,
                            Some(&scope.card),
                        )
                    }
                },
            }
        };
        self.state.agent_effective_surface = surface;
    }

    /// The invocation scope that bounds this Store's current invocation: the primary Store's
    /// admitted scope, or the scope an entity Store inherited from its caller.
    pub(crate) fn current_invocation_scope_card(&self) -> Option<ScopeCard> {
        match &self.runtime {
            OwnerRuntime::Agent => self.state.invocation_scope_card.clone(),
            OwnerRuntime::Entity(_) => self
                .state
                .inherited_invocation_scope
                .as_ref()
                .map(|scope| scope.card.clone()),
        }
    }

    /// Brings a Store's authority up to date with its owner worker at a live boundary: excludes
    /// cards revoked by any of the owner's Stores, removes a scope whose roots they revoked, and
    /// checks an entity Store's inherited scope roots.
    async fn synchronize_owner_authority(&mut self) -> Result<(), WorkerExecutorError> {
        if !self.state.is_live() {
            return Ok(());
        }
        let revoked =
            owner_revoked_card_ids(&*self.public_state.worker().get_last_known_status().await);
        let unchecked_root_ids = self
            .state
            .inherited_invocation_scope
            .as_ref()
            .and_then(InheritedInvocationScope::unchecked_root_ids);
        let root_states = match unchecked_root_ids {
            Some(root_ids) => Some(self.state.card_service.check_cards(root_ids).await?),
            None => None,
        };
        self.apply_owner_authority_synchronization(revoked, root_states)
    }

    fn apply_owner_authority_synchronization(
        &mut self,
        revoked: HashSet<CardId>,
        root_states: Option<HashMap<CardId, CardState>>,
    ) -> Result<(), WorkerExecutorError> {
        let remove_inherited_scope = match &mut self.state.inherited_invocation_scope {
            Some(scope) => {
                let all_live = match &root_states {
                    Some(root_states) => scope.check_roots(root_states)?,
                    None => true,
                };
                !all_live || scope.roots_expired(Utc::now())
            }
            None => false,
        };
        if remove_inherited_scope {
            self.state.inherited_invocation_scope = None;
            if let Some(interest) = &self.state.scope_interest {
                interest.invalidate();
            }
        }
        let revoked_roots = self
            .interested_card_ids()
            .into_iter()
            .filter(|card_id| revoked.contains(card_id))
            .collect::<Vec<_>>();
        self.clear_invocation_scope_if_roots_include(&revoked_roots);
        self.state.owner_revoked_card_ids = revoked;
        self.rederive_agent_effective_surface_from_wallet();
        Ok(())
    }

    pub(in crate::durable_host) fn interested_card_ids(&self) -> Vec<CardId> {
        let mut card_ids = self
            .agent_wallet_cards_snapshot()
            .iter()
            .map(StoredCard::card_id)
            .collect::<BTreeSet<_>>();
        if let Some(scope_card) = &self.state.invocation_scope_card {
            card_ids.extend(scope_card.root_card_ids.iter().copied());
        }
        if let Some(scope) = &self.state.inherited_invocation_scope {
            card_ids.extend(scope.card.root_card_ids.iter().copied());
        }
        if let Some(interest) = &self.state.scope_interest {
            card_ids.extend(interest.roots().iter().copied());
        }
        card_ids.into_iter().collect()
    }

    pub(in crate::durable_host) async fn install_invocation_scope_card(
        &mut self,
        scope_card: Option<ScopeCard>,
        root_cards: Vec<StoredCard>,
        candidate_interest: Option<Arc<crate::services::card_interest::CardInterestContribution>>,
    ) {
        // Invocation scope is part of the effective authority but is not represented by a
        // worker-status generation. Force the first live authorization for this invocation
        // through the synchronized path so the cached expiration deadline includes its roots.
        self.state.authority_initialized = false;
        let (_, handles) = clear_invocation_scope_state(
            &mut self.state.invocation_scope_card,
            &mut self.state.invocation_scope_handles,
        );
        for rep in handles {
            permissions::retire_invocation_scope_handle(self.table(), rep);
        }
        self.state.invocation_scope_card = scope_card;
        self.state.invocation_scope_root_cards = root_cards
            .into_iter()
            .map(|card| (card.card_id(), card))
            .collect();
        self.state.scope_interest = candidate_interest.or_else(|| {
            self.state.invocation_scope_card.as_ref().map(|scope| {
                self.state
                    .owner_wallet
                    .interest
                    .contribute(None, &scope.root_card_ids)
            })
        });
        self.rederive_agent_effective_surface_from_wallet();
    }

    pub(in crate::durable_host) async fn clear_invocation_scope_card(&mut self) {
        self.state.authority_initialized = false;
        let (scope_changed, handles) = clear_invocation_scope_state(
            &mut self.state.invocation_scope_card,
            &mut self.state.invocation_scope_handles,
        );
        for rep in handles {
            permissions::retire_invocation_scope_handle(self.table(), rep);
        }
        if scope_changed {
            self.state.invocation_scope_root_cards.clear();
            self.rederive_agent_effective_surface_from_wallet();
        }
        self.state.scope_interest = None;
    }

    pub(in crate::durable_host) fn clear_invocation_scope_if_roots_include(
        &mut self,
        card_ids: &[CardId],
    ) -> bool {
        let interest_removed = self.state.scope_interest.as_ref().is_some_and(|interest| {
            if interest.roots_include(card_ids) {
                interest.invalidate();
                true
            } else {
                false
            }
        });
        let inherited_scope_removed = self
            .state
            .inherited_invocation_scope
            .as_ref()
            .is_some_and(|scope| scope.roots_include(&card_ids.iter().copied().collect()));
        if inherited_scope_removed {
            self.state.inherited_invocation_scope = None;
        }
        let (scope_changed, handles) = remove_invocation_scope_for_revoked_roots(
            &mut self.state.invocation_scope_card,
            &mut self.state.invocation_scope_handles,
            card_ids,
        );
        for rep in handles {
            permissions::retire_invocation_scope_handle(self.table(), rep);
        }
        if scope_changed {
            self.state.invocation_scope_root_cards.clear();
        }
        scope_changed || inherited_scope_removed || interest_removed
    }

    async fn drain_card_events_at_boundary(&mut self) -> Result<(), WorkerExecutorError> {
        if !self.state.is_live() {
            return Ok(());
        }

        let owner = self.state.owner_wallet.clone();
        let boundary = match self.state.card_boundary.upgrade() {
            Some(boundary) => boundary,
            None => owner.lock_boundary().await?,
        };
        owner
            .synchronize_locked(
                &self.public_state.worker(),
                boundary.clone(),
                self.entity_parent_start_index(),
            )
            .await?;
        self.state.card_boundary = Arc::downgrade(&boundary);
        self.remove_expired_invocation_scope_roots().await?;
        self.state.authority_wallet = owner.snapshot();
        self.synchronize_owner_authority().await
    }

    async fn remove_expired_invocation_scope_roots(&mut self) -> Result<(), WorkerExecutorError> {
        let expired_root_ids =
            expired_wallet_card_ids_at(self.state.invocation_scope_root_cards.values(), Utc::now());
        if !expired_root_ids.is_empty() {
            self.apply_card_revoked_cascade(
                self.entity_parent_start_index(),
                &expired_root_ids,
                true,
            )
            .await?;
        }
        Ok(())
    }

    /// Returns the queued card events that are not applied yet.
    ///
    /// The published agent status is the only source. Outside snapshotting, each producer of a
    /// queued event or of its terminal entry commits it and folds it into the status before it
    /// completes, so the status read under the card event boundary lock is exact and no oplog
    /// entry is read here.
    async fn pending_card_events_at_boundary(&self) -> Vec<PendingCardEventRef> {
        self.public_state
            .worker()
            .get_last_known_status()
            .await
            .pending_card_events
            .clone()
    }

    pub(crate) async fn apply_card_revoked_cascade(
        &mut self,
        entity_parent_start_index: Option<OplogIndex>,
        card_ids: &[CardId],
        commit_immediately: bool,
    ) -> Result<(), WorkerExecutorError> {
        let mut card_ids = card_ids.to_vec();
        card_ids.sort_unstable();
        card_ids.dedup();
        if card_ids.is_empty() {
            return Ok(());
        }

        if self.state.is_live() {
            self.commit_wallet_entry(OplogEntry::CardRevokedCascade {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index,
                revoked_card_ids: card_ids.clone(),
                affected_wallets: Vec::new(),
                local_wallet_generation: 0,
            })
            .await?;
            self.clear_invocation_scope_if_roots_include(&card_ids);
            self.rederive_agent_effective_surface_from_wallet();
            return Ok(());
        }

        let wallet_changed = remove_wallet_cards(
            &mut self.state.agent_wallet_cards,
            &mut self.state.wallet_generation,
            &card_ids,
        )?;
        let scope_changed = self.clear_invocation_scope_if_roots_include(&card_ids);
        if wallet_changed || scope_changed {
            self.rederive_agent_effective_surface_from_wallet();
        }

        let affected_wallets = if wallet_changed {
            vec![CardHolder::Agent(AgentCardHolder {
                agent_id: self.owned_agent_id.agent_id.clone(),
            })]
        } else {
            Vec::new()
        };
        let entry = OplogEntry::CardRevokedCascade {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index,
            revoked_card_ids: card_ids,
            affected_wallets,
            local_wallet_generation: self.state.wallet_generation,
        };
        if commit_immediately {
            self.public_state
                .worker()
                .add_and_commit_oplog(entry)
                .await?;
        } else {
            self.public_state.worker().add_to_oplog(entry).await?;
        }

        Ok(())
    }
}

impl InheritedInvocationScope {
    pub(in crate::durable_host) fn unchecked(card: ScopeCard) -> Self {
        Self { card, roots: None }
    }

    pub(in crate::durable_host) fn unchecked_root_ids(&self) -> Option<Vec<CardId>> {
        self.roots
            .is_none()
            .then(|| self.card.root_card_ids.clone())
    }

    pub(in crate::durable_host) fn live_roots(&self) -> Option<&BTreeMap<CardId, StoredCard>> {
        self.roots.as_ref()
    }

    /// Records the checked root cards and returns whether every root is live. As in the primary
    /// Store after replay, a root that the card service does not know is an error.
    pub(in crate::durable_host) fn check_roots(
        &mut self,
        card_states: &HashMap<CardId, CardState>,
    ) -> Result<bool, WorkerExecutorError> {
        if self.roots.is_some() {
            return Ok(true);
        }
        let roots = live_scope_root_cards_from_states(Some(&self.card), card_states)?;
        let all_live = roots.len() == self.card.root_card_ids.len();
        self.roots = Some(roots);
        Ok(all_live)
    }

    pub(in crate::durable_host) fn roots_include(&self, card_ids: &HashSet<CardId>) -> bool {
        self.card
            .root_card_ids
            .iter()
            .any(|root_id| card_ids.contains(root_id))
    }

    pub(in crate::durable_host) fn roots_expired(&self, now: DateTime<Utc>) -> bool {
        self.roots
            .as_ref()
            .is_some_and(|roots| !expired_wallet_card_ids_at(roots.values(), now).is_empty())
    }
}

pub(crate) fn agent_effective_surface_from_component_metadata(
    component: &Component,
    owned_agent_id: &OwnedAgentId,
    agent_id: &ParsedAgentId,
) -> Result<golem_common::model::card::EffectiveSurface, WorkerExecutorError> {
    let context = agent_monomorphization_context(component, owned_agent_id, agent_id);
    let card = agent_initial_card_from_component_metadata(component, agent_id)?;
    Ok(golem_common::model::card::agent_effective_surface_from_wallet(&context, [&card]))
}

pub(crate) fn owner_effective_surface_from_component_metadata(
    component: &Component,
    owned_agent_id: &OwnedAgentId,
    owner_context: &ResolvedOwnerContext,
) -> Result<golem_common::model::card::EffectiveSurface, WorkerExecutorError> {
    match owner_context {
        ResolvedOwnerContext::Agent(agent_id) => {
            agent_effective_surface_from_component_metadata(component, owned_agent_id, agent_id)
        }
        ResolvedOwnerContext::ComponentWorker | ResolvedOwnerContext::ComponentBaseline => {
            let card = StoredCard::Polymorphic(
                component
                    .metadata
                    .component_provision_config()
                    .initial_permissions
                    .clone(),
            );
            Ok(component_baseline_effective_surface_from_wallet(
                component,
                owned_agent_id,
                [&card],
            ))
        }
    }
}

pub(crate) fn agent_monomorphization_context(
    component: &Component,
    owned_agent_id: &OwnedAgentId,
    agent_id: &ParsedAgentId,
) -> golem_common::model::card::AgentPermissionMonomorphizationContext {
    golem_common::model::card::AgentPermissionMonomorphizationContext {
        account: component.account_email.clone(),
        application: component.application_name.clone(),
        environment: component.environment_name.clone(),
        component: component.component_name.clone(),
        agent_name: owned_agent_id.agent_id.agent_id.clone(),
        owner: golem_common::model::card::recipient::RecipientOwnerContext::AgentType(
            agent_id.agent_type.clone(),
        ),
    }
}

fn component_baseline_monomorphization_context(
    component: &Component,
    owned_agent_id: &OwnedAgentId,
) -> golem_common::model::card::AgentPermissionMonomorphizationContext {
    golem_common::model::card::AgentPermissionMonomorphizationContext {
        account: component.account_email.clone(),
        application: component.application_name.clone(),
        environment: component.environment_name.clone(),
        component: component.component_name.clone(),
        agent_name: owned_agent_id.agent_id.agent_id.clone(),
        owner:
            golem_common::model::card::recipient::RecipientOwnerContext::ComponentExternalToolOwner,
    }
}

pub(crate) fn owner_monomorphization_context(
    component: &Component,
    owned_agent_id: &OwnedAgentId,
    owner_context: &ResolvedOwnerContext,
) -> golem_common::model::card::AgentPermissionMonomorphizationContext {
    match owner_context {
        ResolvedOwnerContext::Agent(agent_id) => {
            agent_monomorphization_context(component, owned_agent_id, agent_id)
        }
        ResolvedOwnerContext::ComponentWorker | ResolvedOwnerContext::ComponentBaseline => {
            component_baseline_monomorphization_context(component, owned_agent_id)
        }
    }
}

pub(in crate::durable_host) fn component_baseline_effective_surface_from_wallet<'a>(
    component: &Component,
    owned_agent_id: &OwnedAgentId,
    cards: impl IntoIterator<Item = &'a StoredCard>,
) -> golem_common::model::card::EffectiveSurface {
    let context = component_baseline_monomorphization_context(component, owned_agent_id);
    golem_common::model::card::agent_effective_surface_from_wallet(&context, cards)
}

pub(crate) fn agent_initial_card_from_component_metadata(
    component: &Component,
    agent_id: &ParsedAgentId,
) -> Result<StoredCard, WorkerExecutorError> {
    let card = component
        .metadata
        .agent_type_initial_permission_card(&agent_id.agent_type)
        .cloned()
        .ok_or_else(|| missing_agent_initial_card_error(component, agent_id))?;
    Ok(StoredCard::Polymorphic(card))
}

fn missing_agent_initial_card_error(
    component: &Component,
    agent_id: &ParsedAgentId,
) -> WorkerExecutorError {
    WorkerExecutorError::invalid_request(format!(
        "Missing initial permission card for agent type {} in component {} revision {}",
        agent_id.agent_type, component.id, component.revision
    ))
}

pub(in crate::durable_host) fn derive_permission_uuid(
    context: &'static str,
    owned_agent_id: &OwnedAgentId,
    invocation_key: &IdempotencyKey,
    oplog_index: OplogIndex,
) -> Uuid {
    derive_permission_uuid_for_sequence(
        context,
        owned_agent_id,
        invocation_key,
        oplog_index.as_u64(),
    )
}

pub(in crate::durable_host) fn derive_permission_uuid_for_sequence(
    context: &'static str,
    owned_agent_id: &OwnedAgentId,
    invocation_key: &IdempotencyKey,
    sequence: u64,
) -> Uuid {
    let agent_name = owned_agent_id.agent_id.agent_id.as_bytes();
    let invocation_key = invocation_key.value.as_bytes();
    let mut hasher = blake3::Hasher::new_derive_key(context);
    hasher.update(owned_agent_id.environment_id.0.as_bytes());
    hasher.update(owned_agent_id.agent_id.component_id.0.as_bytes());
    hasher.update(&(agent_name.len() as u64).to_be_bytes());
    hasher.update(agent_name);
    hasher.update(&(invocation_key.len() as u64).to_be_bytes());
    hasher.update(invocation_key);
    hasher.update(&sequence.to_be_bytes());

    let mut bytes = [0_u8; 16];
    let timestamp = sequence.min(UUID_V7_MAX_TIMESTAMP);
    bytes[..6].copy_from_slice(&timestamp.to_be_bytes()[2..]);
    bytes[6..].copy_from_slice(&hasher.finalize().as_bytes()[..10]);
    bytes[6] = (bytes[6] & 0x0f) | 0x70;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

pub(crate) fn card_holder_is_agent(holder: &CardHolder, agent_id: &AgentId) -> bool {
    matches!(holder, CardHolder::Agent(holder) if holder.agent_id == *agent_id)
}

pub(crate) fn authorize_effective_surface(
    surface: &golem_common::model::card::EffectiveSurface,
    targets: &[PermissionTarget],
) -> Result<(), AuthorizationError> {
    for target in targets {
        let allowed = surface.authorize(target).map_err(|error| {
            AuthorizationError::PermissionEvaluationFailed {
                target: Box::new(target.clone()),
                error,
            }
        })?;
        if !allowed {
            return Err(AuthorizationError::PermissionNotAllowed(Box::new(
                target.clone(),
            )));
        }
    }
    Ok(())
}

pub(crate) fn authority_snapshot_is_current_at(
    initialized: bool,
    authority_open: bool,
    processed_generation: u64,
    published_generation: u64,
    next_expiration: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> bool {
    initialized
        && authority_open
        && processed_generation == published_generation
        && next_expiration.is_none_or(|expiration| now < expiration)
}

pub(in crate::durable_host) fn assert_live_authorization(is_live: bool) {
    assert!(
        is_live,
        "live permission authorization must not run during replay"
    );
}

pub(crate) fn record_permission_decisions(targets: &[PermissionTarget], allowed: bool) {
    if allowed {
        for target in targets {
            crate::metrics::wasm::record_agent_permission_authorization(target.class_name(), true);
        }
    } else {
        // Multi-target operations are all-or-nothing. The authorization API intentionally
        // exposes only a low-cardinality class metric, never the denied resource.
        let permission_class = targets
            .first()
            .map(PermissionTarget::class_name)
            .unwrap_or("unknown");
        crate::metrics::wasm::record_agent_permission_authorization(permission_class, false);
    }
}

pub(in crate::durable_host) fn clear_invocation_scope_state(
    scope_card: &mut Option<ScopeCard>,
    scope_handles: &mut HashMap<u32, Vec<CardId>>,
) -> (bool, HashSet<u32>) {
    let scope_changed = scope_card.take().is_some();
    let handles = std::mem::take(scope_handles).into_keys().collect();
    (scope_changed, handles)
}

pub(in crate::durable_host) fn remove_invocation_scope_for_revoked_roots(
    scope_card: &mut Option<ScopeCard>,
    scope_handles: &mut HashMap<u32, Vec<CardId>>,
    revoked_card_ids: &[CardId],
) -> (bool, HashSet<u32>) {
    let affected = scope_card.as_ref().is_some_and(|scope_card| {
        scope_card
            .root_card_ids
            .iter()
            .any(|root_id| revoked_card_ids.contains(root_id))
    });
    if affected {
        *scope_card = None;
    }
    let handles = scope_handles
        .iter()
        .filter_map(|(rep, root_card_ids)| {
            root_card_ids
                .iter()
                .any(|root_id| revoked_card_ids.contains(root_id))
                .then_some(*rep)
        })
        .collect::<HashSet<_>>();
    for rep in &handles {
        scope_handles.remove(rep);
    }
    (affected, handles)
}

pub(crate) fn owner_revoked_card_ids(
    status: &golem_common::model::AgentStatusRecord,
) -> HashSet<CardId> {
    status
        .revoked_cards
        .iter()
        .copied()
        .chain(
            status
                .pending_card_events
                .iter()
                .filter_map(|pending_event| match &pending_event.event {
                    QueuedCardEvent::Revoke(event) => Some(event.card_id),
                    QueuedCardEvent::Install(_)
                    | QueuedCardEvent::TransferStarted(_)
                    | QueuedCardEvent::TransferReceived(_) => None,
                }),
        )
        .collect()
}

pub(in crate::durable_host) fn live_scope_root_cards_from_states(
    scope_card: Option<&ScopeCard>,
    card_states: &HashMap<CardId, CardState>,
) -> Result<BTreeMap<CardId, StoredCard>, WorkerExecutorError> {
    let Some(scope_card) = scope_card else {
        return Ok(BTreeMap::new());
    };

    let mut root_cards = BTreeMap::new();
    for card_id in &scope_card.root_card_ids {
        match card_states.get(card_id) {
            Some(CardState::Live(card)) if card.card_id() == *card_id => {
                root_cards.insert(*card_id, card.as_ref().clone());
            }
            Some(CardState::Revoked) => {}
            Some(CardState::Live(_)) | Some(CardState::Unknown) | None => {
                return Err(WorkerExecutorError::runtime(format!(
                    "scope-card root {card_id} could not be re-validated after replay"
                )));
            }
        }
    }
    Ok(root_cards)
}

pub(crate) fn transfer_started_removes_source_membership(
    source_card: Option<&StoredCard>,
    source_holder: &CardHolder,
    agent_id: &AgentId,
) -> bool {
    matches!(source_card, Some(StoredCard::Concrete(_)))
        && card_holder_is_agent(source_holder, agent_id)
}

pub(crate) fn next_drainable_card_events(
    pending_events: Vec<PendingCardEventRef>,
) -> Vec<PendingCardEventRef> {
    let mut result = Vec::new();
    let mut collecting_revocations = false;

    for event in pending_events {
        match &event.event {
            QueuedCardEvent::TransferStarted(_) => continue,
            QueuedCardEvent::Install(_) | QueuedCardEvent::TransferReceived(_) => {
                if result.is_empty() {
                    result.push(event);
                }
                break;
            }
            QueuedCardEvent::Revoke(_) => {
                if result.is_empty() {
                    collecting_revocations = true;
                }
                if collecting_revocations {
                    result.push(event);
                } else {
                    break;
                }
            }
        }
    }

    result
}

pub(crate) fn add_wallet_card(
    wallet: &mut WalletCards,
    generation: &mut u64,
    card: StoredCard,
) -> Result<bool, WorkerExecutorError> {
    let card_id = card.card_id();
    if let Some(existing) = wallet.get(&card_id) {
        return if existing.as_ref() == &card {
            Ok(false)
        } else {
            Err(WorkerExecutorError::runtime(format!(
                "wallet card {card_id} conflicts with its existing payload"
            )))
        };
    }
    *generation = generation
        .checked_add(1)
        .ok_or_else(|| WorkerExecutorError::runtime("wallet generation exhausted"))?;
    wallet.insert(card_id, Arc::new(card));
    Ok(true)
}

pub(crate) fn remove_wallet_card(
    wallet: &mut WalletCards,
    generation: &mut u64,
    card_id: CardId,
) -> Result<bool, WorkerExecutorError> {
    if !wallet.contains_key(&card_id) {
        return Ok(false);
    }
    *generation = generation
        .checked_add(1)
        .ok_or_else(|| WorkerExecutorError::runtime("wallet generation exhausted"))?;
    wallet.remove(&card_id);
    Ok(true)
}

pub(crate) fn remove_wallet_cards(
    wallet: &mut WalletCards,
    generation: &mut u64,
    card_ids: &[CardId],
) -> Result<bool, WorkerExecutorError> {
    if !card_ids.iter().any(|card_id| wallet.contains_key(card_id)) {
        return Ok(false);
    }
    let next_generation = generation
        .checked_add(1)
        .ok_or_else(|| WorkerExecutorError::runtime("wallet generation exhausted"))?;

    for card_id in card_ids {
        if wallet.contains_key(card_id) {
            wallet.remove(card_id);
        }
    }
    *generation = next_generation;
    Ok(true)
}

pub(in crate::durable_host) fn expired_wallet_card_ids_at<'a>(
    cards: impl IntoIterator<Item = &'a StoredCard>,
    now: DateTime<Utc>,
) -> Vec<CardId> {
    cards
        .into_iter()
        .filter_map(|card| {
            card.expires_at()
                .filter(|expires_at| *expires_at <= now)
                .map(|_| card.card_id())
        })
        .collect()
}

pub(crate) fn apply_invocation_wallet_pin(
    wallet: &mut WalletCards,
    wallet_id_hash: [u8; 32],
    generation: &mut u64,
    wallet_pin: InvocationWalletPin,
) -> Result<bool, WorkerExecutorError> {
    if wallet_pin.wallet_token.wallet_id_hash != wallet_id_hash {
        return Err(WorkerExecutorError::unexpected_oplog_entry(
            "invocation wallet pin for the local wallet",
            "wallet identity hash does not match the replaying agent",
        ));
    }
    if wallet_pin
        .pinned_card_ids
        .windows(2)
        .any(|card_ids| card_ids[0] >= card_ids[1])
    {
        return Err(WorkerExecutorError::unexpected_oplog_entry(
            "canonically ordered invocation wallet pin",
            "pinned card ids are not strictly increasing",
        ));
    }
    if wallet_pin.wallet_token.generation < *generation {
        return Err(WorkerExecutorError::unexpected_oplog_entry(
            "non-decreasing invocation wallet generation",
            format!(
                "pinned generation {} is behind replayed generation {}",
                wallet_pin.wallet_token.generation, *generation
            ),
        ));
    }

    // Validate the complete pin before changing membership. Retained prefixes share every
    // unchanged node and payload; rebuilding the pinned map would copy all membership nodes.
    if wallet
        .keys()
        .copied()
        .eq(wallet_pin.pinned_card_ids.iter().copied())
    {
        *generation = wallet_pin.wallet_token.generation;
        return Ok(false);
    }
    for card_id in &wallet_pin.pinned_card_ids {
        if !wallet.contains_key(card_id) {
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                "invocation wallet pin backed by replayed card definitions",
                format!("pinned card {card_id} is missing from the replayed wallet"),
            ));
        }
    }

    let removed = wallet
        .keys()
        .filter(|id| wallet_pin.pinned_card_ids.binary_search(id).is_err())
        .copied()
        .collect::<Vec<_>>();
    for card_id in &removed {
        wallet.remove(card_id);
    }
    *generation = wallet_pin.wallet_token.generation;
    Ok(!removed.is_empty())
}

pub(crate) fn adopt_recorded_wallet_generation(
    generation: &mut u64,
    recorded_generation: u64,
) -> Result<(), WorkerExecutorError> {
    if recorded_generation < *generation {
        return Err(WorkerExecutorError::unexpected_oplog_entry(
            "non-decreasing wallet generation",
            format!(
                "recorded generation {recorded_generation} is behind replayed generation {}",
                *generation
            ),
        ));
    }
    *generation = recorded_generation;
    Ok(())
}

pub(crate) fn replace_wallet_cards(
    wallet: &mut WalletCards,
    generation: &mut u64,
    replacement: WalletCards,
) -> Result<bool, WorkerExecutorError> {
    let removed_ids = wallet
        .keys()
        .filter(|card_id| !replacement.contains_key(card_id))
        .copied()
        .collect::<Vec<_>>();
    let added = replacement
        .values()
        .filter(|card| !wallet.contains_key(&card.card_id()))
        .count();
    let change_count = removed_ids
        .len()
        .checked_add(added)
        .and_then(|count| u64::try_from(count).ok())
        .ok_or_else(|| WorkerExecutorError::runtime("wallet generation exhausted"))?;
    if change_count == 0 {
        return Ok(false);
    }
    let next_generation = generation
        .checked_add(change_count)
        .ok_or_else(|| WorkerExecutorError::runtime("wallet generation exhausted"))?;

    for card_id in removed_ids {
        wallet.remove(&card_id);
    }
    for (card_id, card) in replacement {
        if !wallet.contains_key(&card_id) {
            wallet.insert(card_id, card);
        }
    }
    *generation = next_generation;
    Ok(true)
}
