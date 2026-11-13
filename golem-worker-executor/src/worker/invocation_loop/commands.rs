use super::*;

impl PendingFilesystemLimitUpdate {
    pub(super) fn push(
        &mut self,
        allocated_bytes: u64,
        sender: Sender<Result<(), WorkerExecutorError>>,
    ) {
        self.allocated_bytes = allocated_bytes;
        self.senders.push(sender);
    }

    pub(super) fn complete(self, result: Result<(), WorkerExecutorError>) {
        for sender in self.senders {
            let _ = sender.send(result.clone());
        }
    }
}

pub(super) fn coalesce_filesystem_limit_update(
    allocated_bytes: u64,
    sender: Sender<Result<(), WorkerExecutorError>>,
    receiver: &mut UnboundedReceiver<WorkerCommand>,
    deferred_wakeups: &mut VecDeque<WorkerCommand>,
) -> PendingFilesystemLimitUpdate {
    let mut update = PendingFilesystemLimitUpdate {
        allocated_bytes,
        senders: vec![sender],
    };
    let pending_wakeups = std::mem::take(deferred_wakeups);
    let mut absorb = |command| match command {
        WorkerCommand::UpdateFilesystemLimit {
            allocated_bytes,
            sender,
        } => update.push(allocated_bytes, sender),
        command => deferred_wakeups.push_back(command),
    };
    for command in pending_wakeups {
        absorb(command);
    }
    while let Ok(command) = receiver.try_recv() {
        absorb(command);
    }
    update
}

pub(super) fn coalesce_delayed_retry_prefix<Ctx: WorkerCtx>(
    receiver: &mut UnboundedReceiver<WorkerCommand>,
    deferred_wakeups: &mut VecDeque<WorkerCommand>,
    prefix_len: usize,
) -> DelayedRetryCommands {
    let mut commands = DelayedRetryCommands::default();
    let mut work_available = false;

    for _ in 0..prefix_len {
        let Ok(command) = receiver.try_recv() else {
            break;
        };
        match command {
            WorkerCommand::WorkAvailable => work_available = true,
            WorkerCommand::InternalStatusChanged => {}
            WorkerCommand::UpdateFilesystemLimit { sender, .. } => {
                commands.filesystem_limit_updates.push(sender);
            }
        }
    }

    if work_available {
        InvocationLoop::<Ctx>::defer_wakeup(deferred_wakeups, WorkerCommand::WorkAvailable);
    }

    commands
}

pub(super) fn apply_delayed_retry_commands_after_unload(commands: DelayedRetryCommands) {
    for sender in commands.filesystem_limit_updates {
        let _ = sender.send(Ok(()));
    }
}

impl<Ctx: WorkerCtx> InvocationLoop<Ctx> {
    pub(super) fn defer_wakeup(
        deferred_wakeups: &mut VecDeque<WorkerCommand>,
        command: WorkerCommand,
    ) {
        let already_deferred = match &command {
            WorkerCommand::WorkAvailable => deferred_wakeups
                .iter()
                .any(|command| matches!(command, WorkerCommand::WorkAvailable)),
            WorkerCommand::InternalStatusChanged => true,
            WorkerCommand::UpdateFilesystemLimit { .. } => false,
        };

        if !already_deferred {
            deferred_wakeups.push_back(command);
        }
    }
}

impl<Ctx: WorkerCtx> Invocation<'_, Ctx> {
    /// Performs a directory listing command on the worker's file system
    ///
    /// These are threaded through the invocation loop to make sure they are not accessing the file system concurrently with invocations
    /// that may modify them.
    pub(super) async fn get_file_system_node(
        &self,
        path: CanonicalFilePath,
        sender: Sender<Result<GetFileSystemNodeResult, WorkerExecutorError>>,
    ) {
        let _filesystem_access = match self
            .store
            .data()
            .durable_ctx()
            .acquire_owner_filesystem_inspection()
            .await
        {
            Ok(access) => access,
            Err(error) => {
                let _ = sender.send(Err(error));
                return;
            }
        };
        let result = self.store.data().get_file_system_node(&path).await;
        let _ = sender.send(result);
    }

    /// Performs a read file command on the worker's file system
    ///
    /// These are threaded through the invocation loop to make sure they are not accessing the file system concurrently with invocations
    /// that may modify them.
    pub(super) async fn read_file(
        &self,
        path: CanonicalFilePath,
        selection: FileByteSelection,
        mut sender: tokio::sync::oneshot::Sender<Result<FileReadResponse, FileReadError>>,
    ) {
        let access = tokio::select! {
            biased;
            _ = sender.closed() => return,
            result = self.store.data().durable_ctx().acquire_owner_filesystem_inspection() => {
                result.map_err(|_| FileReadError::Lifecycle)
            }
        };
        let _filesystem_access = match access {
            Ok(access) => access,
            Err(error) => {
                let _ = sender.send(Err(error));
                return;
            }
        };
        let generation = self
            .store
            .data()
            .durable_ctx()
            .filesystem_generation_handle();
        crate::services::agent_filesystem::produce_file_read(
            &generation,
            path.as_abs_str(),
            selection,
            sender,
        )
        .await;
    }
}
