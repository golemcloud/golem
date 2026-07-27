use super::*;

/// Live-only abandoned durable-call records tolerated by the invocation-boundary positional read
/// ([`ReplayState::get_oplog_entry_agent_invocation_finished`]).
///
/// Live execution can make partial durable progress that replay legitimately never reproduces:
/// guest-side races (e.g. an HTTP body read racing a zero-length timer) are resolved by the
/// component runtime's scheduling at a granularity the oplog does not record, so a branch that
/// issued durable calls live — and was then abandoned by the guest — may never be re-issued on
/// replay. Those records (`Start`s no replayed call ever claims, plus the `End`/`Cancelled`
/// terminals that closed them) are dead by the time the invocation-finished marker is read: the
/// replayed guest has already produced its invocation result and nothing can claim them anymore.
///
/// The tolerance is deliberately structural and local to a single finished-marker read:
/// - only `Start`s whose committed consume is replay-inert are drained — a
///   dedicated-positional-consumer pair with commit-side replay effects (`GolemApiFork`) stays
///   fatal (see [`AbandonedStarts::can_drain`]);
/// - every drained `Start` must be closed by exactly one `End`/`Cancelled` before
///   `AgentInvocationFinished` — an unclosed or doubly-closed record stays fatal;
/// - terminals of `Start`s not drained by the same walk stay fatal;
/// - every other positional entry stays fatal;
/// - the tracker never survives past the finished-marker read, so a terminal leaking past
///   `AgentInvocationFinished` (a settlement-ordering bug at its producer) is not normalized
///   into accepted history.
#[derive(Default)]
pub(super) struct AbandonedStarts {
    starts: HashMap<OplogIndex, AbandonedStart>,
}

struct AbandonedStart {
    function_name: HostFunctionName,
    parent_start_index: Option<OplogIndex>,
    terminal: Option<(&'static str, OplogIndex)>,
}

impl AbandonedStarts {
    /// Whether a never-claimed `Start` for `function_name` may be drained as live-only abandoned
    /// progress at the invocation boundary.
    ///
    /// `GolemApiFork` is excluded: it is a dedicated-positional-consumer pair whose committed
    /// consume is not inert — [`CursorTx::apply_commit_effects`] records the `Start` as a pending
    /// fork and decodes its matching `End` into a [`ReplayEvent::ForkReplayed`]. Draining such a
    /// pair here would apply a fork the replayed guest never requested, so it stays fatal. Every
    /// other `Start`/`End` commit is side-effect-free (the `End` arm of `apply_commit_effects`
    /// only fires for pending fork starts), so draining them is genuinely inert. Any new
    /// function-specific commit effect added to `apply_commit_effects` must be excluded here too.
    pub(super) fn can_drain(function_name: &HostFunctionName) -> bool {
        !matches!(function_name, HostFunctionName::GolemApiFork)
    }

    pub(super) fn contains(&self, start_index: OplogIndex) -> bool {
        self.starts.contains_key(&start_index)
    }

    pub(super) fn record_start(
        &mut self,
        idx: OplogIndex,
        function_name: HostFunctionName,
        parent_start_index: Option<OplogIndex>,
    ) {
        self.starts.insert(
            idx,
            AbandonedStart {
                function_name,
                parent_start_index,
                terminal: None,
            },
        );
    }

    pub(super) fn record_terminal(
        &mut self,
        start_index: OplogIndex,
        terminal_idx: OplogIndex,
        kind: &'static str,
    ) -> Result<(), WorkerExecutorError> {
        let start = self.starts.get_mut(&start_index).ok_or_else(|| {
            WorkerExecutorError::runtime(format!(
                "abandoned-record tracker has no Start for terminal {kind} at {terminal_idx} \
                 (start_index {start_index})"
            ))
        })?;
        if let Some((prior_kind, prior_idx)) = &start.terminal {
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                "at most one terminal per abandoned Start",
                format!(
                    "{kind} at {terminal_idx} closing abandoned Start {start_index} ({:?}) \
                     already closed by {prior_kind} at {prior_idx}",
                    start.function_name
                ),
            ));
        }
        start.terminal = Some((kind, terminal_idx));
        Ok(())
    }

    /// Validates that every drained abandoned `Start` is terminally closed and emits a single
    /// summary warning for the tolerated records. Called when the boundary walk reaches
    /// `AgentInvocationFinished`.
    pub(super) fn finish(self, owned_agent_id: &OwnedAgentId) -> Result<(), WorkerExecutorError> {
        if self.starts.is_empty() {
            return Ok(());
        }

        let open: Vec<_> = self
            .starts
            .iter()
            .filter(|(_, start)| start.terminal.is_none())
            .map(|(idx, start)| format!("{idx} ({:?})", start.function_name))
            .collect();
        if !open.is_empty() {
            return Err(WorkerExecutorError::unexpected_oplog_entry(
                "an End/Cancelled closing every abandoned Start before AgentInvocationFinished",
                format!(
                    "AgentInvocationFinished with unclosed abandoned Start(s) at {}",
                    open.join(", ")
                ),
            ));
        }

        let mut records: Vec<_> = self.starts.iter().collect();
        records.sort_by_key(|(idx, _)| **idx);
        let roots = records
            .iter()
            .filter(|(_, start)| {
                start
                    .parent_start_index
                    .is_none_or(|parent| !self.starts.contains_key(&parent))
            })
            .count();
        let ended = records
            .iter()
            .filter(|(_, start)| matches!(start.terminal, Some(("End", _))))
            .count();
        let summary: Vec<_> = records
            .iter()
            .map(|(idx, start)| {
                format!(
                    "{idx}: {:?} (parent: {:?}, terminal: {:?})",
                    start.function_name, start.parent_start_index, start.terminal
                )
            })
            .collect();
        warn!(
            "replay of {owned_agent_id} skipped {} abandoned durable-call record(s) \
             ({roots} root(s), {ended} closed by End, {} by Cancelled) at the invocation \
             boundary — live-only progress the replayed guest abandoned earlier: [{}]",
            records.len(),
            records.len() - ended,
            summary.join("; ")
        );
        Ok(())
    }
}
