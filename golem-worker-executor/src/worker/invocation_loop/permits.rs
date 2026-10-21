use super::*;

pub(super) async fn acquire_permit_or_interrupt<T, F: Future<Output = T>>(
    interrupts: &Interrupts,
    receiver: &mut UnboundedReceiver<WorkerCommand>,
    mut acquire: impl FnMut() -> F,
    mut defer: impl FnMut(WorkerCommand),
) -> Result<PermitAcquisition<T>, WorkerExecutorError> {
    let mut terminal = interrupts.terminal();
    'acquire: loop {
        if let Some(interrupt) = take_pending_interrupt(interrupts).await {
            return Ok(PermitAcquisition::Interrupted(interrupt));
        }
        let permit = acquire();
        tokio::pin!(permit);
        loop {
            tokio::select! {
                biased;
                pending = async { terminal.wait_for(|pending| *pending).await.map(|_| ()) } => {
                    pending.map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
                    // Drop the scheduler wait before joining this request's establishment.
                    continue 'acquire;
                }
                command = receiver.recv() => {
                    let Some(command) = command else {
                        return Ok(PermitAcquisition::CommandChannelClosed);
                    };
                    defer(command);
                    if interrupts.has_interrupt().await {
                        continue 'acquire;
                    }
                }
                permit = &mut permit => return Ok(PermitAcquisition::Acquired(permit)),
            }
        }
    }
}

impl<Ctx: WorkerCtx> InnerInvocationLoop<'_, Ctx> {
    /// Release the concurrent-agent permit back to the semaphore pool.
    /// Called when the agent enters idle state. No-op if already released.
    pub(super) async fn release_concurrent_agent_permit(
        &mut self,
    ) -> Result<(), WorkerExecutorError> {
        if self.permit_state.is_some() {
            debug!(agent_id = %self.owned_agent_id.agent_id, "Releasing concurrent-agent permit (entering idle)");
            if let Some(window) = self.permit_state.take_window() {
                let result = window
                    .close(resource_usage_close_deadline())
                    .await
                    .map(|_| ())
                    .map_err(|error| WorkerExecutorError::runtime(error.to_string()));
                self.permit_state.mark_released();
                return result;
            }
            self.permit_state.release();
        }
        Ok(())
    }

    /// Re-acquire the concurrent-agent permit from the scheduler.
    /// Called when the agent wakes from idle to process a command.
    /// The scheduler ensures FIFO ordering within the account so that a worker
    /// that just finished goes to the back of the queue.
    pub(super) async fn acquire_concurrent_agent_permit(
        &mut self,
    ) -> Result<PermitAcquisition<()>, WorkerExecutorError> {
        if self.permit_state.is_none() {
            let span = agent_phase_span!(self, "acquire_concurrent_agent_permit");
            let agent_id = self.owned_agent_id.agent_id().clone();
            let registered_concurrent_account = self.parent.registered_concurrent_account.clone();
            let deferred_wakeups = &mut *self.deferred_wakeups;
            let acquisition = acquire_permit_or_interrupt(
                &self.interrupts,
                self.receiver,
                || {
                    let account = registered_concurrent_account.clone();
                    let agent_id = agent_id.clone();
                    async move {
                        debug!("Re-acquiring concurrent-agent permit (waking from idle)");
                        account.acquire(agent_id).await
                    }
                },
                |command| InvocationLoop::<Ctx>::defer_wakeup(deferred_wakeups, command),
            )
            .instrument(span)
            .await?;
            let permit = match acquisition {
                PermitAcquisition::Acquired(permit) => permit,
                PermitAcquisition::Interrupted(interrupt) => {
                    return Ok(PermitAcquisition::Interrupted(interrupt));
                }
                PermitAcquisition::CommandChannelClosed => {
                    return Ok(PermitAcquisition::CommandChannelClosed);
                }
            };
            let permit = self.permit_state.track(permit);
            match crate::services::agent_filesystem::open_resource_usage_window(
                self.filesystem,
                permit,
            )
            .await
            {
                Ok(window) => self.permit_state.install_window(
                    ExecutionWindow::new(&self.parent, window, self.start_attempt).await,
                ),
                Err(error) => {
                    self.permit_state.release_scope.fail(
                        crate::metrics::resource_release::Stage::EndToEnd,
                        error.release_failure(),
                    );
                    self.permit_state.mark_released();
                    return Err(WorkerExecutorError::runtime(error.to_string()));
                }
            }
        }
        Ok(PermitAcquisition::Acquired(()))
    }
}

impl<T> ConcurrentAgentPermitState<T> {
    pub fn new(permit: Option<T>, held: Arc<AtomicBool>) -> Self {
        held.store(permit.is_some(), Ordering::Release);
        Self {
            permit,
            window: None,
            held,
            release_scope: Default::default(),
        }
    }

    pub fn with_release_scope(
        mut self,
        scope: crate::metrics::resource_release::ReleaseScope,
    ) -> Self {
        self.release_scope = scope;
        self
    }

    pub(super) fn is_some(&self) -> bool {
        self.permit.is_some() || self.window.is_some()
    }

    pub(super) fn is_none(&self) -> bool {
        self.permit.is_none() && self.window.is_none()
    }

    pub(super) fn install(&mut self, permit: T) {
        debug_assert!(self.permit.is_none());
        self.permit = Some(permit);
        self.held.store(true, Ordering::Release);
    }

    pub(super) fn take_permit(&mut self) -> Option<T> {
        self.permit.take()
    }

    pub(super) fn install_window(&mut self, window: ExecutionWindow) {
        debug_assert!(self.permit.is_none());
        debug_assert!(self.window.is_none());
        self.release_scope = window.release_scope.clone();
        self.window = Some(window);
        self.held.store(true, Ordering::Release);
    }

    pub(super) fn take_window(&mut self) -> Option<ExecutionWindow> {
        self.window.take()
    }

    pub(super) fn mark_released(&self) {
        self.held.store(false, Ordering::Release);
    }

    pub(super) fn release(&mut self) {
        if let Some(permit) = self.permit.take() {
            drop(permit);
            self.held.store(false, Ordering::Release);
        } else if let Some(window) = self.window.take() {
            drop(window);
        }
    }
}

impl ConcurrentAgentPermitState<crate::services::active_agents::ConcurrentAgentPermit> {
    pub(super) fn track(
        &self,
        permit: crate::services::active_agents::ConcurrentAgentPermit,
    ) -> crate::services::active_agents::ConcurrentAgentPermit {
        permit.track_held(Arc::clone(&self.held))
    }

    pub(super) fn install_tracked(
        &mut self,
        permit: crate::services::active_agents::ConcurrentAgentPermit,
    ) {
        self.install(permit.track_held(Arc::clone(&self.held)));
    }
}

pub(super) fn resource_usage_close_deadline() -> Instant {
    Instant::now() + Duration::from_secs(30)
}

pub(super) fn mark_idle(idle_since_millis: &AtomicU64) {
    let now = Timestamp::now_utc().to_millis();
    let _ = idle_since_millis.try_update(Ordering::Release, Ordering::Acquire, |previous| {
        Some(now.max(previous.saturating_add(1)))
    });
}
