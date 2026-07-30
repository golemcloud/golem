use super::*;

impl ReplayState {
    /// Drops a resolver awaiter from outside a cursor transaction. Acquires the cursor lock briefly
    /// (on an owned task; callers are accessor futures); callers must not hold it (the await loop
    /// releases it before parking).
    async fn unregister_awaiter(&self, start_idx: OplogIndex) {
        let result = self
            .run_owned_cursor_op(move |state| async move {
                let mut st = state.cursor.state.lock().await;
                st.concurrent_resolver.unregister(start_idx);
                Ok(())
            })
            .await;
        if let Err(err) = result {
            warn!("unregister_awaiter cursor operation did not complete: {err}");
        }
    }

    /// Drains every *awaited terminal* (`End`/`Cancelled` whose `start_index` has a registered
    /// awaiter) currently at the cursor head, routing each to its awaiter, then stops at the first
    /// non-terminal entry without consuming it. This is the cursor-driving half of
    /// [`Self::await_resolution_outcome`]; it never blocks (it parks by returning, not suspending).
    pub(super) async fn drain_awaited_terminals(&self) -> Result<(), WorkerExecutorError> {
        self.run_owned_cursor_op(|state| async move {
            // `|_| false` never matches a non-terminal, so the transaction only auto-drains the
            // awaited terminals at the head and then returns `None` on the first non-terminal
            // entry (or at end-of-replay) without consuming it.
            state
                .with_tx(async |tx| tx.try_get_oplog_entry(|_| false).await)
                .await
                .map(|_| ())
        })
        .await
    }

    /// Delivery-time validation of a resolved outcome: a `CompletedButDiscarded` resolution whose
    /// marker lies *beyond* the effective replay target is an invalid replay configuration — the
    /// target falls between the call's successful `End` and its `CompletionDiscarded` marker, so
    /// the delivery status of the `End` cannot be decided from the visible oplog prefix. Debug
    /// target validation and cut-point validation reject such targets up front; this check is
    /// defense in depth for any other path that bounds replay between the two entries. Future
    /// knowledge (the marker beyond the target) is only ever used to *reject* the target, never
    /// to decide a call's outcome within it.
    fn validate_resolved_outcome(
        &self,
        outcome: ResolutionOutcome,
    ) -> Result<ResolutionOutcome, WorkerExecutorError> {
        if let ResolutionOutcome::Resolved(Resolution::CompletedButDiscarded {
            end_idx,
            marker_idx,
            ..
        }) = &outcome
        {
            let target = self.replay_target();
            if *marker_idx > target {
                return Err(WorkerExecutorError::invalid_request(format!(
                    "invalid replay target {target}: it lies between a durable call's successful End at {end_idx} and its CompletionDiscarded marker at {marker_idx}, so the delivery status of the completion is undecidable at this target"
                )));
            }
        }
        Ok(outcome)
    }

    /// Awaits the resolution of the call identified by `handle`, treating end-of-replay as a hard
    /// error (the caller requires the call to have completed in the oplog).
    pub async fn await_resolution(
        &self,
        handle: ReplayCallHandle,
    ) -> Result<Resolution, WorkerExecutorError> {
        let start_idx = handle.start_idx();
        match self.await_resolution_outcome(handle).await? {
            ResolutionOutcome::Resolved(resolution) => Ok(resolution),
            ResolutionOutcome::Incomplete => Err(WorkerExecutorError::unexpected_oplog_entry(
                "End or Cancelled",
                format!(
                    "end of replay: durable call Start at {start_idx} has no matching End/Cancelled"
                ),
            )),
        }
    }

    /// Awaits the resolution of the call identified by `handle`, reporting a lone committed `Start`
    /// (replay reached the end of the oplog without the matching `End`/`Cancelled`) as
    /// [`ResolutionOutcome::Incomplete`] rather than a hard error, so the caller can decide whether
    /// to re-execute the call.
    ///
    /// This is the genuine concurrent-replay suspend/resume path. The awaiter does not drive the
    /// cursor toward its own `End` directly; instead it repeatedly:
    /// 1. drains the awaited terminals at the cursor head ([`Self::drain_awaited_terminals`]) —
    ///    resolving this call (when its `End` is at the head) and, in the interleaved case, routing
    ///    earlier-completing siblings' terminals to their own awaiters;
    /// 2. checks its receiver;
    /// 3. if still unresolved and replay is not over, **suspends** until either its resolution
    ///    arrives (a concurrently-replaying sibling drove the cursor to this call's `End`) or the
    ///    cursor advances (a positional consumer or a sibling claim made progress past the blocker),
    ///    then loops.
    ///
    /// Cursor progress is registered (`Notified::enable`) *before* the cursor is inspected, so a
    /// progress signal racing the inspection is never lost. The cursor lock is released before the
    /// suspension (the drain takes and drops it), so other in-flight calls can drive the cursor
    /// while this one sleeps — which is what lets overlapping calls' `End`s, recorded in a
    /// non-deterministic completion order, replay out of claim order.
    pub async fn await_resolution_outcome(
        &self,
        handle: ReplayCallHandle,
    ) -> Result<ResolutionOutcome, WorkerExecutorError> {
        let (start_idx, mut receiver) = handle.into_parts();
        let validate = |outcome: ResolutionOutcome| self.validate_resolved_outcome(outcome);

        loop {
            // Register interest in cursor progress before inspecting the cursor, so a signal that
            // races the inspection below is delivered to the suspension at the end of the loop.
            let progress = self.cursor.progress.notified();
            tokio::pin!(progress);
            progress.as_mut().enable();

            // Drain the terminals at the head: resolves this call in the serial case, and any
            // already-claimed, earlier-completing sibling in the interleaved case.
            self.drain_awaited_terminals().await?;

            match receiver.try_recv() {
                Ok(outcome) => return validate(outcome),
                Err(oneshot::error::TryRecvError::Empty) => {}
                Err(oneshot::error::TryRecvError::Closed) => {
                    // Sender dropped without resolving (anomalous). Drop any lingering registration.
                    self.unregister_awaiter(start_idx).await;
                    return Err(WorkerExecutorError::runtime(format!(
                        "concurrent replay resolver channel closed for Start at {start_idx}"
                    )));
                }
            }

            if self.is_live() {
                // The lock-free `is_live()` snapshot may have observed a sibling transaction that
                // already published `last_replayed_index == replay_target` while committing *this*
                // call's terminal but had not yet routed it to the resolver (delivery in
                // `on_committed_replay_entry` happens after the position is published, see
                // `commit_consumed_entry`). Acquire the cursor lock — serializing with any such
                // in-flight transaction — and re-check the receiver before concluding the call is
                // incomplete, so a just-resolved final terminal is never misreported. The lock is
                // taken on an owned task (this is an accessor future; see `run_owned_cursor_op`),
                // which owns the receiver for the duration of the check; every branch is terminal,
                // so the receiver never needs to be handed back.
                let outcome = self
                    .run_owned_cursor_op(move |state| async move {
                        let mut st = state
                            .cursor
                            .state.lock()
                            .await;
                        match receiver.try_recv() {
                            Ok(outcome) => Ok(outcome),
                            Err(oneshot::error::TryRecvError::Empty) => {
                                // Genuinely reached the end of the oplog without the matching
                                // `End`/`Cancelled`: a committed lone `Start` (a forced commit
                                // flushed it before its `End`, or a crash happened in between).
                                // Drop the stale registration and report Incomplete so the caller
                                // can re-execute the side effect and complete the existing `Start`.
                                st.concurrent_resolver.unregister(start_idx);
                                Ok(ResolutionOutcome::Incomplete)
                            }
                            Err(oneshot::error::TryRecvError::Closed) => {
                                st.concurrent_resolver.unregister(start_idx);
                                Err(WorkerExecutorError::runtime(format!(
                                    "concurrent replay resolver channel closed for Start at {start_idx}"
                                )))
                            }
                        }
                    })
                    .await?;
                return match outcome {
                    ResolutionOutcome::Incomplete => Ok(ResolutionOutcome::Incomplete),
                    resolved => validate(resolved),
                };
            }

            // This call's terminal is not at the cursor head and replay is not over: a
            // concurrently-replaying sibling owns the cursor head. Suspend until our resolution
            // arrives or the cursor advances, then re-drive.
            tokio::select! {
                biased;
                resolved = &mut receiver => {
                    return match resolved {
                        Ok(outcome) => validate(outcome),
                        Err(_closed) => {
                            self.unregister_awaiter(start_idx).await;
                            Err(WorkerExecutorError::runtime(format!(
                                "concurrent replay resolver channel closed for Start at {start_idx}"
                            )))
                        }
                    };
                }
                _ = progress.as_mut() => {
                    // The cursor advanced; loop to re-drain and re-check.
                }
            }
        }
    }
}
