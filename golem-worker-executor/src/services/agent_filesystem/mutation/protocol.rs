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
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NativeFilesystemError {
    kind: std::io::ErrorKind,
    raw_os_error: Option<i32>,
    message: String,
}

impl NativeFilesystemError {
    fn capture(error: &std::io::Error) -> Self {
        Self {
            kind: error.kind(),
            raw_os_error: error.raw_os_error(),
            message: error.to_string(),
        }
    }

    pub(crate) fn kind(&self) -> std::io::ErrorKind {
        self.kind
    }

    pub(crate) fn raw_os_error(&self) -> Option<i32> {
        self.raw_os_error
    }

    pub(crate) fn into_io_error(self) -> std::io::Error {
        self.raw_os_error.map_or_else(
            || std::io::Error::new(self.kind, self.message),
            std::io::Error::from_raw_os_error,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AgentFilesystemMutationError {
    Native {
        error: NativeFilesystemError,
        completed: u64,
    },
    QuotaExhausted {
        error: NativeFilesystemError,
        completed: u64,
    },
    InsufficientSpace {
        error: NativeFilesystemError,
        completed: u64,
    },
    Cancelled {
        completed: u64,
    },
    RuntimeInvalidated {
        error: Option<NativeFilesystemError>,
        completed: Option<u64>,
    },
}

pub(crate) type AgentFilesystemMutationResult = Result<u64, AgentFilesystemMutationError>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AgentFilesystemWriteMode {
    Position(u64),
    Append,
}

#[derive(Clone)]
pub(crate) struct AgentFilesystemMutations {
    runtime: AgentFilesystemRuntime,
}

impl AgentFilesystemMutations {
    pub(super) fn new(runtime: AgentFilesystemRuntime) -> Self {
        Self { runtime }
    }

    pub(crate) fn writer(
        &self,
        file: File,
        mode: AgentFilesystemWriteMode,
    ) -> AgentFilesystemWriter {
        AgentFilesystemWriter::new(
            self.clone(),
            Arc::new(NativeFilesystemWriter { file }),
            mode,
            WriteCompletion::Fill,
        )
    }

    pub(crate) fn positioned_write(
        &self,
        file: File,
        offset: u64,
        contents: Bytes,
    ) -> Result<AgentFilesystemWriteCompletion, AgentFilesystemMutationError> {
        self.positioned_write_with_native(
            Arc::new(NativeFilesystemWriter { file }),
            offset,
            contents,
        )
    }

    fn positioned_write_with_native(
        &self,
        native: Arc<dyn FilesystemWriter>,
        offset: u64,
        contents: Bytes,
    ) -> Result<AgentFilesystemWriteCompletion, AgentFilesystemMutationError> {
        AgentFilesystemWriter::new(
            self.clone(),
            native,
            AgentFilesystemWriteMode::Position(offset),
            WriteCompletion::FirstSuccess,
        )
        .admit(contents)
        .map(|write| write.execute(tokio_util::sync::CancellationToken::new()))
    }

    pub(crate) async fn resize(&self, file: File, size: u64) -> AgentFilesystemMutationResult {
        self.resize_with_native(Arc::new(NativeFilesystemResize { file }), size)
            .await
    }

    async fn resize_with_native(
        &self,
        native: Arc<dyn FilesystemResize>,
        size: u64,
    ) -> AgentFilesystemMutationResult {
        let effect = self.runtime.begin_update_effect().await.map_err(|_| {
            AgentFilesystemMutationError::RuntimeInvalidated {
                error: None,
                completed: None,
            }
        })?;
        let before = match native.state().await {
            Ok(before) => before,
            Err(error) => {
                return self
                    .classify_probe_failure(MutationOperation::Resize, error)
                    .await;
            }
        };
        let effect = Arc::new(effect);
        let started = Instant::now();
        let mut failures = 0;

        loop {
            let result = native.resize(size, Arc::clone(&effect)).await;
            let Err(error) = result else {
                return Ok(0);
            };
            failures += 1;
            let native_error = NativeFilesystemError::capture(&error);
            let postcondition = resize_postcondition(before, native.state().await, size);
            let mutation_effect = match postcondition {
                MutationPostcondition::Satisfied => MutationEffect::DesiredPostconditionSatisfied,
                MutationPostcondition::NoEffect => MutationEffect::ProvenNoEffect,
                MutationPostcondition::Unknown => MutationEffect::Unknown,
            };
            let decision = self
                .runtime
                .classify_mutation_failure_for::<()>(
                    MutationOperation::Resize,
                    MutationFailure::Io(error),
                    mutation_effect,
                )
                .await;
            match resolve_decision(
                &self.runtime,
                decision,
                native_error,
                0,
                failures,
                started,
                false,
                MutationOperation::Resize,
            )
            .await
            {
                DecisionResolution::Retry => {}
                DecisionResolution::Complete(result) => return result,
            }
        }
    }

    async fn classify_probe_failure(
        &self,
        operation: MutationOperation,
        error: std::io::Error,
    ) -> AgentFilesystemMutationResult {
        let native_error = NativeFilesystemError::capture(&error);
        match self
            .runtime
            .classify_mutation_failure_for::<()>(
                operation,
                MutationFailure::Io(error),
                MutationEffect::ProvenNoEffect,
            )
            .await
        {
            MutationDecision::Quota => Err(AgentFilesystemMutationError::QuotaExhausted {
                error: native_error,
                completed: 0,
            }),
            MutationDecision::InsufficientSpace | MutationDecision::PhysicalPressure => {
                Err(AgentFilesystemMutationError::InsufficientSpace {
                    error: native_error,
                    completed: 0,
                })
            }
            MutationDecision::Invalidate => Err(AgentFilesystemMutationError::RuntimeInvalidated {
                error: Some(native_error),
                completed: Some(0),
            }),
            MutationDecision::PreserveGuest(())
            | MutationDecision::BoundedRetry
            | MutationDecision::PreserveRaw => Err(AgentFilesystemMutationError::Native {
                error: native_error,
                completed: 0,
            }),
            MutationDecision::Success => unreachable!("failed probe cannot satisfy a mutation"),
        }
    }

    #[cfg(test)]
    fn writer_with_native(
        &self,
        native: Arc<dyn FilesystemWriter>,
        mode: AgentFilesystemWriteMode,
    ) -> AgentFilesystemWriter {
        AgentFilesystemWriter::new(self.clone(), native, mode, WriteCompletion::Fill)
    }

    #[cfg(test)]
    async fn resize_with_scripted_native(
        &self,
        native: Arc<dyn FilesystemResize>,
        size: u64,
    ) -> AgentFilesystemMutationResult {
        self.resize_with_native(native, size).await
    }
}

pub(crate) struct AgentFilesystemWriter {
    mutations: AgentFilesystemMutations,
    native: Arc<dyn FilesystemWriter>,
    state: Arc<tokio::sync::Mutex<WriterState>>,
    sequence: Arc<WriterSequence>,
    completion: WriteCompletion,
}

#[derive(Clone, Copy)]
enum WriteCompletion {
    Fill,
    FirstSuccess,
}

struct WriterState {
    mode: AgentFilesystemWriteMode,
}

struct WriterSequence {
    state: std::sync::Mutex<WriterSequenceState>,
    advanced: tokio::sync::Notify,
}

struct WriterSequenceState {
    next_admission: u64,
    next_execution: u64,
    skipped: std::collections::BTreeSet<u64>,
}

impl AgentFilesystemWriter {
    fn new(
        mutations: AgentFilesystemMutations,
        native: Arc<dyn FilesystemWriter>,
        mode: AgentFilesystemWriteMode,
        completion: WriteCompletion,
    ) -> Self {
        Self {
            mutations,
            native,
            state: Arc::new(tokio::sync::Mutex::new(WriterState { mode })),
            sequence: Arc::new(WriterSequence {
                state: std::sync::Mutex::new(WriterSequenceState {
                    next_admission: 0,
                    next_execution: 0,
                    skipped: std::collections::BTreeSet::new(),
                }),
                advanced: tokio::sync::Notify::new(),
            }),
            completion,
        }
    }

    pub(crate) fn admit(
        &self,
        contents: Bytes,
    ) -> Result<AdmittedFilesystemWrite, AgentFilesystemMutationError> {
        let admission = self.mutations.runtime.admit_effect().map_err(|_| {
            AgentFilesystemMutationError::RuntimeInvalidated {
                error: None,
                completed: None,
            }
        })?;
        Ok(AdmittedFilesystemWrite {
            runtime: self.mutations.runtime.clone(),
            native: Arc::clone(&self.native),
            state: Arc::clone(&self.state),
            ticket: WriterTicket::reserve(&self.sequence),
            contents,
            admission,
            completion: self.completion,
        })
    }
}

pub(crate) struct AdmittedFilesystemWrite {
    runtime: AgentFilesystemRuntime,
    native: Arc<dyn FilesystemWriter>,
    state: Arc<tokio::sync::Mutex<WriterState>>,
    ticket: WriterTicket,
    contents: Bytes,
    admission: AgentFilesystemEffectAdmission,
    completion: WriteCompletion,
}

impl AdmittedFilesystemWrite {
    pub(crate) fn execute(
        self,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> AgentFilesystemWriteCompletion {
        let runtime = self.runtime.clone();
        AgentFilesystemWriteCompletion {
            runtime: runtime.clone(),
            task: tokio::spawn(async move {
                match tokio::spawn(self.run(cancellation)).await {
                    Ok(result) => result,
                    Err(_) => {
                        runtime.invalidate_runtime().await;
                        Err(AgentFilesystemMutationError::RuntimeInvalidated {
                            error: None,
                            completed: None,
                        })
                    }
                }
            }),
        }
    }

    async fn run(
        mut self,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> AgentFilesystemMutationResult {
        if !self.ticket.wait_for_turn(&cancellation).await {
            return Err(AgentFilesystemMutationError::Cancelled { completed: 0 });
        }
        let mut state = tokio::select! {
            state = self.state.lock() => state,
            _ = cancellation.cancelled() => {
                return Err(AgentFilesystemMutationError::Cancelled { completed: 0 });
            }
        };
        let initial_mode = state.mode;
        let effect = tokio::select! {
            effect = async {
                match initial_mode {
                    AgentFilesystemWriteMode::Position(_) => self.admission.begin().await,
                    AgentFilesystemWriteMode::Append => self.admission.begin_append().await,
                }
            } => effect.map_err(|_| AgentFilesystemMutationError::RuntimeInvalidated {
                error: None,
                completed: None,
            })?,
            _ = cancellation.cancelled() => {
                return Err(AgentFilesystemMutationError::Cancelled { completed: 0 });
            }
        };
        let effect = Arc::new(effect);
        let started = Instant::now();
        let mut completed = 0usize;
        let mut failures = 0;

        while completed < self.contents.len() {
            if cancellation.is_cancelled() {
                let completed = completed_u64(completed);
                advance_position_or_invalidate(&self.runtime, &mut state, completed).await?;
                return Err(AgentFilesystemMutationError::Cancelled { completed });
            }
            let mode = match attempt_mode(initial_mode, completed) {
                Ok(mode) => mode,
                Err(error) => {
                    self.runtime.invalidate_runtime().await;
                    return Err(error);
                }
            };
            let attempt = self
                .native
                .write(mode, self.contents.slice(completed..), Arc::clone(&effect))
                .await;
            let remaining = self.contents.len() - completed;
            if attempt.written > remaining {
                self.runtime.invalidate_runtime().await;
                return Err(AgentFilesystemMutationError::RuntimeInvalidated {
                    error: None,
                    completed: Some(completed_u64(completed)),
                });
            }
            completed += attempt.written;

            let (error, mutation_effect) = match attempt.result {
                Ok(()) if matches!(self.completion, WriteCompletion::FirstSuccess) => {
                    let completed = completed_u64(completed);
                    advance_position_or_invalidate(&self.runtime, &mut state, completed).await?;
                    return Ok(completed);
                }
                Ok(()) if attempt.written != 0 => {
                    if cancellation.is_cancelled() {
                        let completed = completed_u64(completed);
                        advance_position_or_invalidate(&self.runtime, &mut state, completed)
                            .await?;
                        return Err(AgentFilesystemMutationError::Cancelled { completed });
                    }
                    continue;
                }
                Ok(()) => (
                    std::io::Error::from(std::io::ErrorKind::WriteZero),
                    proven_write_progress_effect(completed),
                ),
                Err(error) => {
                    let effect = attempt
                        .failure_effect
                        .unwrap_or_else(|| native_write_failure_effect(&error, completed));
                    (error, effect)
                }
            };
            failures += 1;
            let native_error = NativeFilesystemError::capture(&error);
            let decision = self
                .runtime
                .classify_mutation_failure_for::<()>(
                    MutationOperation::Write,
                    MutationFailure::Io(error),
                    mutation_effect,
                )
                .await;
            let completed_u64 = completed_u64(completed);
            match resolve_decision(
                &self.runtime,
                decision,
                native_error,
                completed_u64,
                failures,
                started,
                cancellation.is_cancelled(),
                MutationOperation::Write,
            )
            .await
            {
                DecisionResolution::Retry => {}
                DecisionResolution::Complete(result) => {
                    advance_position_or_invalidate(&self.runtime, &mut state, completed_u64)
                        .await?;
                    return result;
                }
            }
        }

        let completed = completed_u64(completed);
        advance_position_or_invalidate(&self.runtime, &mut state, completed).await?;
        Ok(completed)
    }
}

pub(crate) struct AgentFilesystemWriteCompletion {
    runtime: AgentFilesystemRuntime,
    task: tokio::task::JoinHandle<AgentFilesystemMutationResult>,
}

impl Future for AgentFilesystemWriteCompletion {
    type Output = AgentFilesystemMutationResult;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.task).poll(context).map(|result| {
            result.unwrap_or_else(|_| {
                self.runtime.seal();
                Err(AgentFilesystemMutationError::RuntimeInvalidated {
                    error: None,
                    completed: None,
                })
            })
        })
    }
}

struct WriterTicket {
    sequence: Arc<WriterSequence>,
    number: u64,
    entered: bool,
}

impl WriterTicket {
    fn reserve(sequence: &Arc<WriterSequence>) -> Self {
        let number = {
            let mut state = sequence
                .state
                .lock()
                .expect("filesystem writer sequence lock poisoned");
            let number = state.next_admission;
            state.next_admission = state
                .next_admission
                .checked_add(1)
                .expect("filesystem writer admission sequence overflowed");
            number
        };
        Self {
            sequence: Arc::clone(sequence),
            number,
            entered: false,
        }
    }

    async fn wait_for_turn(&mut self, cancellation: &tokio_util::sync::CancellationToken) -> bool {
        loop {
            let mut advanced = Box::pin(self.sequence.advanced.notified());
            advanced.as_mut().enable();
            if self
                .sequence
                .state
                .lock()
                .expect("filesystem writer sequence lock poisoned")
                .next_execution
                == self.number
            {
                self.entered = true;
                return true;
            }
            tokio::select! {
                _ = advanced => {}
                _ = cancellation.cancelled() => return false,
            }
        }
    }
}

impl Drop for WriterTicket {
    fn drop(&mut self) {
        let mut state = self
            .sequence
            .state
            .lock()
            .expect("filesystem writer sequence lock poisoned");
        if self.entered {
            debug_assert_eq!(state.next_execution, self.number);
            state.next_execution += 1;
        } else {
            state.skipped.insert(self.number);
        }
        while state.next_execution < state.next_admission {
            let next = state.next_execution;
            if !state.skipped.remove(&next) {
                break;
            }
            state.next_execution += 1;
        }
        drop(state);
        self.sequence.advanced.notify_waiters();
    }
}

struct FilesystemWriteAttempt {
    written: usize,
    result: std::io::Result<()>,
    failure_effect: Option<MutationEffect>,
}

#[async_trait]
trait FilesystemWriter: Send + Sync {
    async fn write(
        &self,
        mode: AgentFilesystemWriteMode,
        contents: Bytes,
        effect: Arc<AgentFilesystemEffectLease>,
    ) -> FilesystemWriteAttempt;
}

#[async_trait]
trait FilesystemResize: Send + Sync {
    async fn state(&self) -> std::io::Result<PathState>;

    async fn resize(
        &self,
        size: u64,
        effect: Arc<AgentFilesystemUpdateEffectLease>,
    ) -> std::io::Result<()>;
}

struct NativeFilesystemResize {
    file: File,
}

#[async_trait]
impl FilesystemResize for NativeFilesystemResize {
    async fn state(&self) -> std::io::Result<PathState> {
        descriptor_state(&Descriptor::File(self.file.clone())).await
    }

    async fn resize(
        &self,
        size: u64,
        effect: Arc<AgentFilesystemUpdateEffectLease>,
    ) -> std::io::Result<()> {
        let file = self.file.clone();
        run_blocking_filesystem_mutation(effect, move || resize_file(&file, size)).await
    }
}

struct NativeFilesystemWriter {
    file: File,
}

#[async_trait]
impl FilesystemWriter for NativeFilesystemWriter {
    async fn write(
        &self,
        mode: AgentFilesystemWriteMode,
        contents: Bytes,
        effect: Arc<AgentFilesystemEffectLease>,
    ) -> FilesystemWriteAttempt {
        let file = Arc::clone(&self.file.file);
        spawn_blocking(move || {
            let _effect = effect;
            let result = match mode {
                AgentFilesystemWriteMode::Position(position) => file.write_at(&contents, position),
                AgentFilesystemWriteMode::Append => {
                    let mut file = file.as_ref();
                    file.seek(SeekFrom::End(0))
                        .and_then(|_| file.write(&contents))
                }
            };
            match result {
                Ok(written) => FilesystemWriteAttempt {
                    written,
                    result: Ok(()),
                    failure_effect: None,
                },
                Err(error) => FilesystemWriteAttempt {
                    written: 0,
                    result: Err(error),
                    failure_effect: None,
                },
            }
        })
        .await
    }
}

enum DecisionResolution {
    Retry,
    Complete(AgentFilesystemMutationResult),
}

async fn resolve_decision(
    runtime: &AgentFilesystemRuntime,
    decision: MutationDecision<()>,
    error: NativeFilesystemError,
    completed: u64,
    failures: usize,
    started: Instant,
    cancelled: bool,
    operation: MutationOperation,
) -> DecisionResolution {
    let within_retry_bound = failures < FILESYSTEM_MUTATION_MAX_ATTEMPTS
        && started.elapsed() <= FILESYSTEM_MUTATION_RETRY_TIMEOUT;
    match decision {
        MutationDecision::BoundedRetry if cancelled => {
            DecisionResolution::Complete(Err(AgentFilesystemMutationError::Cancelled { completed }))
        }
        MutationDecision::BoundedRetry if within_retry_bound => DecisionResolution::Retry,
        MutationDecision::BoundedRetry | MutationDecision::PreserveRaw => {
            DecisionResolution::Complete(Err(AgentFilesystemMutationError::Native {
                error,
                completed,
            }))
        }
        MutationDecision::PreserveGuest(()) => {
            DecisionResolution::Complete(Err(AgentFilesystemMutationError::Native {
                error,
                completed,
            }))
        }
        MutationDecision::Quota => {
            DecisionResolution::Complete(Err(AgentFilesystemMutationError::QuotaExhausted {
                error,
                completed,
            }))
        }
        MutationDecision::InsufficientSpace => {
            DecisionResolution::Complete(Err(AgentFilesystemMutationError::InsufficientSpace {
                error,
                completed,
            }))
        }
        MutationDecision::PhysicalPressure if cancelled => {
            DecisionResolution::Complete(Err(AgentFilesystemMutationError::Cancelled { completed }))
        }
        MutationDecision::PhysicalPressure
            if within_retry_bound
                && runtime
                    .recover_physical_pressure(
                        operation,
                        started + FILESYSTEM_MUTATION_RETRY_TIMEOUT,
                    )
                    .await
                && started.elapsed() <= FILESYSTEM_MUTATION_RETRY_TIMEOUT =>
        {
            DecisionResolution::Retry
        }
        MutationDecision::PhysicalPressure => {
            DecisionResolution::Complete(Err(AgentFilesystemMutationError::InsufficientSpace {
                error,
                completed,
            }))
        }
        MutationDecision::Success => DecisionResolution::Complete(Ok(completed)),
        MutationDecision::Invalidate => {
            DecisionResolution::Complete(Err(AgentFilesystemMutationError::RuntimeInvalidated {
                error: Some(error),
                completed: Some(completed),
            }))
        }
    }
}

fn attempt_mode(
    mode: AgentFilesystemWriteMode,
    completed: usize,
) -> Result<AgentFilesystemWriteMode, AgentFilesystemMutationError> {
    match mode {
        AgentFilesystemWriteMode::Position(position) => {
            let completed = completed_u64(completed);
            position
                .checked_add(completed)
                .map(AgentFilesystemWriteMode::Position)
                .ok_or(AgentFilesystemMutationError::RuntimeInvalidated {
                    error: None,
                    completed: Some(completed),
                })
        }
        AgentFilesystemWriteMode::Append => Ok(AgentFilesystemWriteMode::Append),
    }
}

fn completed_u64(completed: usize) -> u64 {
    u64::try_from(completed).expect("usize must fit in u64 on supported targets")
}

fn advance_position(
    state: &mut WriterState,
    completed: u64,
) -> Result<(), AgentFilesystemMutationError> {
    if let AgentFilesystemWriteMode::Position(position) = &mut state.mode {
        *position = position.checked_add(completed).ok_or(
            AgentFilesystemMutationError::RuntimeInvalidated {
                error: None,
                completed: Some(completed),
            },
        )?;
    }
    Ok(())
}

async fn advance_position_or_invalidate(
    runtime: &AgentFilesystemRuntime,
    state: &mut WriterState,
    completed: u64,
) -> Result<(), AgentFilesystemMutationError> {
    match advance_position(state, completed) {
        Ok(()) => Ok(()),
        Err(error) => {
            runtime.invalidate_runtime().await;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_r::test;

    struct ScriptedFilesystemWriter {
        attempts: std::sync::Mutex<VecDeque<FilesystemWriteAttempt>>,
        calls: std::sync::Mutex<Vec<(AgentFilesystemWriteMode, Vec<u8>)>>,
        started: Option<Arc<tokio::sync::Notify>>,
        release: Option<Arc<tokio::sync::Semaphore>>,
    }

    struct ScriptedFilesystemResize {
        states: std::sync::Mutex<VecDeque<Result<PathState, i32>>>,
        attempts: std::sync::Mutex<VecDeque<Option<i32>>>,
    }

    #[async_trait]
    impl FilesystemResize for ScriptedFilesystemResize {
        async fn state(&self) -> std::io::Result<PathState> {
            self.states
                .lock()
                .unwrap()
                .pop_front()
                .unwrap()
                .map_err(std::io::Error::from_raw_os_error)
        }

        async fn resize(
            &self,
            _size: u64,
            _effect: Arc<AgentFilesystemUpdateEffectLease>,
        ) -> std::io::Result<()> {
            match self.attempts.lock().unwrap().pop_front().unwrap() {
                Some(errno) => Err(std::io::Error::from_raw_os_error(errno)),
                None => Ok(()),
            }
        }
    }

    struct PanickingFilesystemWriter;

    #[async_trait]
    impl FilesystemWriter for PanickingFilesystemWriter {
        async fn write(
            &self,
            _mode: AgentFilesystemWriteMode,
            _contents: Bytes,
            _effect: Arc<AgentFilesystemEffectLease>,
        ) -> FilesystemWriteAttempt {
            panic!("scripted native write panic")
        }
    }

    impl ScriptedFilesystemWriter {
        fn new(attempts: impl IntoIterator<Item = FilesystemWriteAttempt>) -> Self {
            Self {
                attempts: std::sync::Mutex::new(attempts.into_iter().collect()),
                calls: std::sync::Mutex::new(Vec::new()),
                started: None,
                release: None,
            }
        }

        fn blocked(
            attempts: impl IntoIterator<Item = FilesystemWriteAttempt>,
            started: Arc<tokio::sync::Notify>,
            release: Arc<tokio::sync::Semaphore>,
        ) -> Self {
            Self {
                attempts: std::sync::Mutex::new(attempts.into_iter().collect()),
                calls: std::sync::Mutex::new(Vec::new()),
                started: Some(started),
                release: Some(release),
            }
        }

        fn calls(&self) -> Vec<(AgentFilesystemWriteMode, Vec<u8>)> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl FilesystemWriter for ScriptedFilesystemWriter {
        async fn write(
            &self,
            mode: AgentFilesystemWriteMode,
            contents: Bytes,
            _effect: Arc<AgentFilesystemEffectLease>,
        ) -> FilesystemWriteAttempt {
            let index = {
                let mut calls = self.calls.lock().unwrap();
                let index = calls.len();
                calls.push((mode, contents.to_vec()));
                index
            };
            if index == 0 {
                if let Some(started) = &self.started {
                    started.notify_one();
                }
                if let Some(release) = &self.release {
                    release.acquire().await.unwrap().forget();
                }
            }
            self.attempts.lock().unwrap().pop_front().unwrap()
        }
    }

    fn success(written: usize) -> FilesystemWriteAttempt {
        FilesystemWriteAttempt {
            written,
            result: Ok(()),
            failure_effect: None,
        }
    }

    fn failure(written: usize, errno: i32) -> FilesystemWriteAttempt {
        FilesystemWriteAttempt {
            written,
            result: Err(std::io::Error::from_raw_os_error(errno)),
            failure_effect: None,
        }
    }

    fn failure_with_effect(
        written: usize,
        errno: i32,
        effect: MutationEffect,
    ) -> FilesystemWriteAttempt {
        FilesystemWriteAttempt {
            written,
            result: Err(std::io::Error::from_raw_os_error(errno)),
            failure_effect: Some(effect),
        }
    }

    fn writer(
        runtime: &AgentFilesystemRuntime,
        native: Arc<ScriptedFilesystemWriter>,
        mode: AgentFilesystemWriteMode,
    ) -> AgentFilesystemWriter {
        runtime.mutations().writer_with_native(native, mode)
    }

    fn path_state(size: u64) -> PathState {
        PathState {
            identity: None,
            type_: PathObjectType::RegularFile,
            size,
        }
    }

    #[test]
    async fn successful_write_reports_completed_prefix() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let native = Arc::new(ScriptedFilesystemWriter::new([success(5)]));

        let result = writer(
            &runtime,
            Arc::clone(&native),
            AgentFilesystemWriteMode::Position(7),
        )
        .admit(Bytes::from_static(b"hello"))
        .unwrap()
        .execute(tokio_util::sync::CancellationToken::new())
        .await;

        assert_eq!(result, Ok(5));
        assert_eq!(
            native.calls(),
            [(AgentFilesystemWriteMode::Position(7), b"hello".to_vec())]
        );
    }

    #[test]
    async fn positioned_write_preserves_successful_short_write() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let native = Arc::new(ScriptedFilesystemWriter::new([success(2)]));

        let result = runtime
            .mutations()
            .positioned_write_with_native(
                Arc::clone(&native) as Arc<dyn FilesystemWriter>,
                7,
                Bytes::from_static(b"hello"),
            )
            .unwrap()
            .await;

        assert_eq!(result, Ok(2));
        assert_eq!(
            native.calls(),
            [(AgentFilesystemWriteMode::Position(7), b"hello".to_vec())]
        );
    }

    #[test]
    async fn positioned_write_preserves_successful_zero_write() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let native = Arc::new(ScriptedFilesystemWriter::new([success(0)]));

        let result = runtime
            .mutations()
            .positioned_write_with_native(
                Arc::clone(&native) as Arc<dyn FilesystemWriter>,
                7,
                Bytes::from_static(b"hello"),
            )
            .unwrap()
            .await;

        assert_eq!(result, Ok(0));
        assert_eq!(
            native.calls(),
            [(AgentFilesystemWriteMode::Position(7), b"hello".to_vec())]
        );
    }

    #[test]
    async fn semantic_resize_executes_behind_mutation_seam() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("resized");
        std::fs::write(&path, b"hello").unwrap();
        let file = File::new(
            cap_std::fs::File::from_std(
                std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&path)
                    .unwrap(),
            ),
            FilePerms::all(),
            OpenMode::READ | OpenMode::WRITE,
            false,
            path.clone(),
        );

        let result = runtime.mutations().resize(file, 2).await;

        assert_eq!(result, Ok(0));
        assert_eq!(std::fs::read(path).unwrap(), b"he");
    }

    #[test]
    async fn admitted_write_registers_effect_synchronously() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let native = Arc::new(ScriptedFilesystemWriter::new([success(5)]));
        let admitted = writer(&runtime, native, AgentFilesystemWriteMode::Position(0))
            .admit(Bytes::from_static(b"hello"))
            .unwrap();

        assert!(runtime.has_active_effects());
        drop(admitted);
        assert!(!runtime.has_active_effects());
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn satisfied_direct_postcondition_turns_native_failure_into_success() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let native = Arc::new(ScriptedFilesystemResize {
            states: std::sync::Mutex::new(
                [Ok(path_state(5)), Ok(path_state(2))].into_iter().collect(),
            ),
            attempts: std::sync::Mutex::new([Some(libc::EBUSY)].into_iter().collect()),
        });

        let result = runtime
            .mutations()
            .resize_with_scripted_native(native, 2)
            .await;

        assert_eq!(result, Ok(0));
        assert!(runtime.begin_effect().await.is_ok());
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn proven_no_effect_direct_failure_preserves_native_errno() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        runtime.set_retry_callback(Some(Arc::new(|| Box::pin(async { false }))));
        let native = Arc::new(ScriptedFilesystemResize {
            states: std::sync::Mutex::new(
                [Ok(path_state(5)), Ok(path_state(5))].into_iter().collect(),
            ),
            attempts: std::sync::Mutex::new([Some(libc::EBUSY)].into_iter().collect()),
        });

        let result = runtime
            .mutations()
            .resize_with_scripted_native(native, 2)
            .await;

        assert!(matches!(
            result,
            Err(AgentFilesystemMutationError::Native { error, completed: 0 })
                if error.raw_os_error() == Some(libc::EBUSY)
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn terminal_initial_probe_invalidates_runtime() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let native = Arc::new(ScriptedFilesystemResize {
            states: std::sync::Mutex::new([Err(libc::EIO)].into_iter().collect()),
            attempts: std::sync::Mutex::new(VecDeque::new()),
        });

        let result = runtime
            .mutations()
            .resize_with_scripted_native(native, 2)
            .await;

        assert!(matches!(
            result,
            Err(AgentFilesystemMutationError::RuntimeInvalidated { .. })
        ));
        assert!(runtime.begin_effect().await.is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn proven_no_effect_failure_preserves_native_errno() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        runtime.set_retry_callback(Some(Arc::new(|| Box::pin(async { false }))));
        let native = Arc::new(ScriptedFilesystemWriter::new([failure_with_effect(
            0,
            libc::EBUSY,
            MutationEffect::ProvenNoEffect,
        )]));

        let result = writer(&runtime, native, AgentFilesystemWriteMode::Position(0))
            .admit(Bytes::from_static(b"hello"))
            .unwrap()
            .execute(tokio_util::sync::CancellationToken::new())
            .await;

        assert!(matches!(
            result,
            Err(AgentFilesystemMutationError::Native { error, completed: 0 })
                if error.raw_os_error() == Some(libc::EBUSY)
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn partial_prefix_is_settled_and_only_suffix_is_retried() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let native = Arc::new(ScriptedFilesystemWriter::new([
            failure(2, libc::EBUSY),
            success(3),
        ]));

        let result = writer(
            &runtime,
            Arc::clone(&native),
            AgentFilesystemWriteMode::Position(11),
        )
        .admit(Bytes::from_static(b"hello"))
        .unwrap()
        .execute(tokio_util::sync::CancellationToken::new())
        .await;

        assert_eq!(result, Ok(5));
        assert_eq!(
            native.calls(),
            [
                (AgentFilesystemWriteMode::Position(11), b"hello".to_vec()),
                (AgentFilesystemWriteMode::Position(13), b"llo".to_vec()),
            ]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn quota_exhaustion_is_terminal_and_preserves_errno() {
        let capacity = FilesystemCapacity {
            total_bytes: 100,
            available_bytes: 0,
            total_filesystem_objects: 100,
            available_filesystem_objects: 0,
        };
        let runtime = AgentFilesystemRuntime::new_for_test_with_observations(
            Some(AgentFilesystemUsage {
                allocated_bytes: 50,
                filesystem_objects: 1,
            }),
            Some(ResolvedAgentFilesystemLimits {
                allocated_bytes: 50,
                filesystem_objects: 10,
                filesystem_object_limit_policy_version: FILESYSTEM_OBJECT_LIMIT_POLICY_VERSION,
            }),
            capacity,
        );
        let native = Arc::new(ScriptedFilesystemWriter::new([failure(0, libc::ENOSPC)]));

        let result = writer(&runtime, native, AgentFilesystemWriteMode::Position(0))
            .admit(Bytes::from_static(b"hello"))
            .unwrap()
            .execute(tokio_util::sync::CancellationToken::new())
            .await;

        assert!(matches!(
            result,
            Err(AgentFilesystemMutationError::QuotaExhausted { error, completed: 0 })
                if error.raw_os_error() == Some(libc::ENOSPC)
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn physical_pressure_recovers_before_retrying() {
        let capacity = FilesystemCapacity {
            total_bytes: 100,
            available_bytes: 0,
            total_filesystem_objects: 100,
            available_filesystem_objects: 0,
        };
        let runtime = AgentFilesystemRuntime::new_for_test_with_observations(None, None, capacity);
        let recoveries = Arc::new(AtomicUsize::new(0));
        runtime.set_pressure_recovery_callback(Some(Arc::new({
            let recoveries = Arc::clone(&recoveries);
            move |_, _| {
                let recoveries = Arc::clone(&recoveries);
                Box::pin(async move {
                    recoveries.fetch_add(1, Ordering::AcqRel);
                    true
                })
            }
        })));
        let native = Arc::new(ScriptedFilesystemWriter::new([
            failure(0, libc::ENOSPC),
            success(5),
        ]));

        let result = writer(
            &runtime,
            Arc::clone(&native),
            AgentFilesystemWriteMode::Append,
        )
        .admit(Bytes::from_static(b"hello"))
        .unwrap()
        .execute(tokio_util::sync::CancellationToken::new())
        .await;

        assert_eq!(result, Ok(5));
        assert_eq!(recoveries.load(Ordering::Acquire), 1);
        assert_eq!(native.calls().len(), 2);
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn physical_pressure_without_recovery_is_insufficient_space() {
        let capacity = FilesystemCapacity {
            total_bytes: 100,
            available_bytes: 0,
            total_filesystem_objects: 100,
            available_filesystem_objects: 0,
        };
        let runtime = AgentFilesystemRuntime::new_for_test_with_observations(None, None, capacity);
        let native = Arc::new(ScriptedFilesystemWriter::new([failure(0, libc::ENOSPC)]));

        let result = writer(&runtime, native, AgentFilesystemWriteMode::Append)
            .admit(Bytes::from_static(b"hello"))
            .unwrap()
            .execute(tokio_util::sync::CancellationToken::new())
            .await;

        assert!(matches!(
            result,
            Err(AgentFilesystemMutationError::InsufficientSpace { error, completed: 0 })
                if error.raw_os_error() == Some(libc::ENOSPC)
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn retry_exhaustion_preserves_native_errno() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let native = Arc::new(ScriptedFilesystemWriter::new([
            failure(0, libc::EBUSY),
            failure(0, libc::EBUSY),
        ]));

        let result = writer(&runtime, native, AgentFilesystemWriteMode::Position(0))
            .admit(Bytes::from_static(b"hello"))
            .unwrap()
            .execute(tokio_util::sync::CancellationToken::new())
            .await;

        assert!(matches!(
            result,
            Err(AgentFilesystemMutationError::Native { error, completed: 0 })
                if error.raw_os_error() == Some(libc::EBUSY)
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn unknown_effect_invalidates_runtime() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let native = Arc::new(ScriptedFilesystemWriter::new([failure_with_effect(
            0,
            libc::EINTR,
            MutationEffect::Unknown,
        )]));

        let result = writer(&runtime, native, AgentFilesystemWriteMode::Position(0))
            .admit(Bytes::from_static(b"hello"))
            .unwrap()
            .execute(tokio_util::sync::CancellationToken::new())
            .await;

        assert!(matches!(
            result,
            Err(AgentFilesystemMutationError::RuntimeInvalidated {
                completed: Some(0),
                ..
            })
        ));
        assert!(runtime.begin_effect().await.is_err());
    }

    #[test]
    async fn cancellation_during_native_completion_retains_prefix() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let native = Arc::new(ScriptedFilesystemWriter::blocked(
            [success(2)],
            Arc::clone(&started),
            Arc::clone(&release),
        ));
        let cancellation = tokio_util::sync::CancellationToken::new();
        let completion = writer(&runtime, native, AgentFilesystemWriteMode::Position(0))
            .admit(Bytes::from_static(b"hello"))
            .unwrap()
            .execute(cancellation.clone());
        started.notified().await;

        cancellation.cancel();
        assert!(runtime.has_active_effects());
        release.add_permits(1);

        assert_eq!(
            completion.await,
            Err(AgentFilesystemMutationError::Cancelled { completed: 2 })
        );
    }

    #[test]
    async fn dropped_awaiter_keeps_native_completion_owned() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let native = Arc::new(ScriptedFilesystemWriter::blocked(
            [success(5)],
            Arc::clone(&started),
            Arc::clone(&release),
        ));
        let completion = writer(&runtime, native, AgentFilesystemWriteMode::Append)
            .admit(Bytes::from_static(b"hello"))
            .unwrap()
            .execute(tokio_util::sync::CancellationToken::new());
        started.notified().await;
        drop(completion);

        assert!(runtime.has_active_effects());
        release.add_permits(1);
        tokio::time::timeout(std::time::Duration::from_secs(1), runtime.drain())
            .await
            .unwrap();
    }

    #[test]
    async fn native_task_failure_seals_runtime() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let completion = runtime
            .mutations()
            .writer_with_native(
                Arc::new(PanickingFilesystemWriter),
                AgentFilesystemWriteMode::Position(0),
            )
            .admit(Bytes::from_static(b"hello"))
            .unwrap()
            .execute(tokio_util::sync::CancellationToken::new());

        assert!(matches!(
            completion.await,
            Err(AgentFilesystemMutationError::RuntimeInvalidated { .. })
        ));
        assert!(runtime.begin_effect().await.is_err());
    }

    #[test]
    async fn idle_writer_holds_no_effect_admission() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let native = Arc::new(ScriptedFilesystemWriter::new([]));
        let _writer = writer(&runtime, native, AgentFilesystemWriteMode::Position(0));

        assert!(!runtime.has_active_effects());
    }

    #[test]
    async fn cancellation_while_append_admission_waits_never_calls_native() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let first_started = Arc::new(tokio::sync::Notify::new());
        let first_release = Arc::new(tokio::sync::Semaphore::new(0));
        let first_native = Arc::new(ScriptedFilesystemWriter::blocked(
            [success(5)],
            Arc::clone(&first_started),
            Arc::clone(&first_release),
        ));
        let second_native = Arc::new(ScriptedFilesystemWriter::new([success(5)]));
        let first = writer(&runtime, first_native, AgentFilesystemWriteMode::Append)
            .admit(Bytes::from_static(b"first"))
            .unwrap()
            .execute(tokio_util::sync::CancellationToken::new());
        first_started.notified().await;
        let cancellation = tokio_util::sync::CancellationToken::new();
        let second = writer(
            &runtime,
            Arc::clone(&second_native),
            AgentFilesystemWriteMode::Append,
        )
        .admit(Bytes::from_static(b"other"))
        .unwrap()
        .execute(cancellation.clone());

        cancellation.cancel();
        assert_eq!(
            second.await,
            Err(AgentFilesystemMutationError::Cancelled { completed: 0 })
        );
        assert!(second_native.calls().is_empty());
        first_release.add_permits(1);
        assert_eq!(first.await, Ok(5));
    }

    #[test]
    async fn append_coordination_is_shared_across_prepared_writers() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let first_started = Arc::new(tokio::sync::Notify::new());
        let first_release = Arc::new(tokio::sync::Semaphore::new(0));
        let first_native = Arc::new(ScriptedFilesystemWriter::blocked(
            [success(5)],
            Arc::clone(&first_started),
            Arc::clone(&first_release),
        ));
        let second_native = Arc::new(ScriptedFilesystemWriter::new([success(5)]));
        let first = writer(&runtime, first_native, AgentFilesystemWriteMode::Append)
            .admit(Bytes::from_static(b"first"))
            .unwrap()
            .execute(tokio_util::sync::CancellationToken::new());
        first_started.notified().await;
        let second = writer(
            &runtime,
            Arc::clone(&second_native),
            AgentFilesystemWriteMode::Append,
        )
        .admit(Bytes::from_static(b"other"))
        .unwrap()
        .execute(tokio_util::sync::CancellationToken::new());
        tokio::task::yield_now().await;

        assert!(second_native.calls().is_empty());
        first_release.add_permits(1);
        assert_eq!(first.await, Ok(5));
        assert_eq!(second.await, Ok(5));
    }

    #[test]
    async fn admitted_chunks_execute_in_admission_order() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let first_started = Arc::new(tokio::sync::Notify::new());
        let first_release = Arc::new(tokio::sync::Semaphore::new(0));
        let native = Arc::new(ScriptedFilesystemWriter::blocked(
            [success(5), success(5)],
            Arc::clone(&first_started),
            Arc::clone(&first_release),
        ));
        let writer = writer(
            &runtime,
            Arc::clone(&native),
            AgentFilesystemWriteMode::Position(10),
        );
        let first = writer.admit(Bytes::from_static(b"first")).unwrap();
        let second = writer.admit(Bytes::from_static(b"other")).unwrap();
        let second = second.execute(tokio_util::sync::CancellationToken::new());
        let first = first.execute(tokio_util::sync::CancellationToken::new());
        first_started.notified().await;

        assert_eq!(native.calls().len(), 1);
        first_release.add_permits(1);
        assert_eq!(first.await, Ok(5));
        assert_eq!(second.await, Ok(5));
        assert_eq!(
            native.calls(),
            [
                (AgentFilesystemWriteMode::Position(10), b"first".to_vec()),
                (AgentFilesystemWriteMode::Position(15), b"other".to_vec()),
            ]
        );
    }

    #[test]
    async fn dropped_admitted_chunk_does_not_block_later_chunks() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let native = Arc::new(ScriptedFilesystemWriter::new([success(5)]));
        let writer = writer(&runtime, native, AgentFilesystemWriteMode::Position(0));
        let skipped = writer.admit(Bytes::from_static(b"skip!")).unwrap();
        let next = writer.admit(Bytes::from_static(b"hello")).unwrap();

        drop(skipped);

        assert_eq!(
            next.execute(tokio_util::sync::CancellationToken::new())
                .await,
            Ok(5)
        );
    }

    #[test]
    async fn cancellation_while_writer_sequence_waits_releases_admission() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let first_started = Arc::new(tokio::sync::Notify::new());
        let first_release = Arc::new(tokio::sync::Semaphore::new(0));
        let native = Arc::new(ScriptedFilesystemWriter::blocked(
            [success(5)],
            Arc::clone(&first_started),
            Arc::clone(&first_release),
        ));
        let writer = writer(&runtime, native, AgentFilesystemWriteMode::Position(0));
        let first = writer
            .admit(Bytes::from_static(b"first"))
            .unwrap()
            .execute(tokio_util::sync::CancellationToken::new());
        first_started.notified().await;
        let cancellation = tokio_util::sync::CancellationToken::new();
        let second = writer
            .admit(Bytes::from_static(b"other"))
            .unwrap()
            .execute(cancellation.clone());

        cancellation.cancel();
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), second)
                .await
                .unwrap(),
            Err(AgentFilesystemMutationError::Cancelled { completed: 0 })
        );
        assert!(runtime.has_active_effects());
        first_release.add_permits(1);
        assert_eq!(first.await, Ok(5));
        runtime.drain().await;
    }
}
