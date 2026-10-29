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

impl<Adapter: SandboxFilesystemAdapter> FilesystemGenerationHandle<Adapter> {
    /// Gives one body its own admission and drainage boundary without sealing its siblings.
    pub(crate) fn for_invocation(&self) -> (Self, FilesystemInvocation) {
        let invocation = FilesystemInvocation {
            activity: Arc::new(InvocationActivity {
                state: Mutex::new(InvocationActivityState {
                    closed: false,
                    active: 0,
                }),
                changed: tokio::sync::Notify::new(),
            }),
        };
        let mut handle = self.clone();
        handle.invocation = Some(invocation.clone());
        (handle, invocation)
    }

    pub(crate) fn invocation(&self) -> Option<FilesystemInvocation> {
        self.invocation.clone()
    }

    pub(super) fn lease_call(
        &self,
        registry: &Arc<GenerationRegistry>,
        effect: CallEffect,
    ) -> Result<CallLease, AccessError> {
        let invocation = self
            .invocation
            .as_ref()
            .map(FilesystemInvocation::admit)
            .transpose()?;
        let mut lease = registry.lease_call(effect)?;
        lease.invocation = invocation;
        Ok(lease)
    }

    pub(crate) fn attach_usage_tracker(
        &self,
        tracker: &crate::services::linear_memory::LinearMemoryTracker,
        filesystem_capable: bool,
    ) -> Result<(), AccessError> {
        let generation = self.generation.upgrade().ok_or(AccessError::Revoked)?;
        if let Some(activation) = &generation.usage_activation {
            tracker.set_live_activation(activation.with_filesystem(filesystem_capable));
        }
        Ok(())
    }
}

impl<Adapter: SandboxFilesystemAdapter> Clone for FilesystemGenerationHandle<Adapter> {
    fn clone(&self) -> Self {
        Self {
            generation: self.generation.clone(),
            phase: self.phase,
            invocation: self.invocation.clone(),
        }
    }
}

impl<Stage: FilesystemStage, Adapter: SandboxFilesystemAdapter> Drop
    for AgentFilesystem<Stage, Adapter>
{
    fn drop(&mut self) {
        let Some(generation) = self.generation.take() else {
            return;
        };
        self.stage.take();
        generation.registry.seal();
        spawn_cleanup(generation, None);
    }
}

#[cfg(test)]
pub(crate) fn create_fresh(
    provisioning: SandboxFilesystemProvisioning,
    scratch: Arc<HostDirectory>,
    agent: OwnedAgentId,
    limits: ResolvedStorageLimits,
) -> impl Future<Output = Result<CreatedFilesystem, CreateFailure>> + Send + 'static {
    create_fresh_with::<SandboxFilesystem>(provisioning, scratch, agent, limits)
}

/// Creates an empty sandbox filesystem and returns it in the `Created` stage.
///
/// `AgentFilesystems` calls this at worker startup with the generation's resolved limits and write
/// pressure recovery. Each capture and each restore of the generation uses its own directory in
/// `scratch`. Creation or agent-name validation failures are returned as `CreateFailure`.
pub fn create_fresh_with_pressure_recovery(
    provisioning: SandboxFilesystemProvisioning,
    scratch: Arc<HostDirectory>,
    agent: OwnedAgentId,
    limits: ResolvedStorageLimits,
    pressure_recovery: FilesystemWriteRecovery,
) -> impl Future<Output = Result<CreatedFilesystem, CreateFailure>> + Send + 'static {
    create_fresh_with_recovery::<SandboxFilesystem>(
        provisioning,
        scratch,
        agent,
        limits,
        Some(pressure_recovery),
    )
}

#[cfg(test)]
fn create_fresh_with<Adapter: SandboxFilesystemAdapter>(
    provisioning: Adapter::Provisioning,
    scratch: Arc<HostDirectory>,
    agent: OwnedAgentId,
    limits: ResolvedStorageLimits,
) -> impl Future<Output = Result<CreatedFilesystem<Adapter>, CreateFailure>> + Send + 'static {
    create_fresh_with_recovery::<Adapter>(provisioning, scratch, agent, limits, None)
}

/// Creates a generation with the accounting policy of its sandbox.
pub(super) async fn create_fresh_with_recovery<Adapter: SandboxFilesystemAdapter>(
    provisioning: Adapter::Provisioning,
    scratch: Arc<HostDirectory>,
    agent: OwnedAgentId,
    limits: ResolvedStorageLimits,
    pressure_recovery: Option<FilesystemWriteRecovery>,
) -> Result<CreatedFilesystem<Adapter>, CreateFailure> {
    let name = SandboxFilesystemName::new(
        agent.environment_id.to_string(),
        agent.agent_id.component_id.to_string(),
        agent_path_segment(&agent.agent_id),
    )
    .map_err(|source| CreateFailure { source })?;
    let sandbox = Adapter::create_fresh(provisioning, name, limits.sandbox())
        .await
        .map_err(|source| CreateFailure { source })?;
    let allocation_reader = sandbox.allocation_reader();
    let accounting = sandbox.agent_accounting();
    Ok(AgentFilesystem {
        generation: Some(Arc::new(FilesystemGeneration {
            usage_activation: None,
            release_metrics: std::sync::OnceLock::new(),
            sandbox: tokio::sync::RwLock::new(Some(Arc::new(sandbox))),
            allocation_reader,
            registry: Arc::new(GenerationRegistry::new()),
            limits: Mutex::new(limits),
            initial_files: Mutex::new(Arc::default()),
            initial_file_updates: tokio::sync::Mutex::new(()),
            pressure_recovery,
            accounting,
            namespace: Arc::new(NamespaceCoordinator::new()),
            scratch,
        })),
        stage: Some(Created),
    })
}

#[cfg(test)]
pub(crate) fn bind_resource_usage_metering<Adapter: SandboxFilesystemAdapter>(
    filesystem: CreatedFilesystem<Adapter>,
    account: ResourceUsageAccount,
) -> Result<ReconstructingFilesystem<Adapter>, MeteringBindFailure<Adapter>> {
    bind_configured_resource_usage_metering(
        filesystem,
        account,
        crate::services::golem_config::ResourceUsageMeteringConfig::all_enabled(),
    )
}

/// Binds configured resource metering and advances a `Created` filesystem to `Reconstructing`.
///
/// Worker startup calls this before opening a metering window or materializing initial files.
/// Filesystem usage is installed only when enabled. Failure returns the original `Created`
/// filesystem for cleanup and reports `RuntimeInvalidated` if exclusive ownership was lost.
pub(crate) fn bind_configured_resource_usage_metering<Adapter: SandboxFilesystemAdapter>(
    filesystem: CreatedFilesystem<Adapter>,
    account: ResourceUsageAccount,
    config: crate::services::golem_config::ResourceUsageMeteringConfig,
) -> Result<ReconstructingFilesystem<Adapter>, MeteringBindFailure<Adapter>> {
    let (generation, _) = filesystem.into_parts();
    let mut generation = match Arc::try_unwrap(generation) {
        Ok(generation) => generation,
        Err(generation) => {
            let filesystem = AgentFilesystem {
                generation: Some(generation),
                stage: Some(Created),
            };
            return Err(MeteringBindFailure {
                filesystem,
                source: Error::RuntimeInvalidated,
            });
        }
    };
    let meter = create_unbound_meter(config, account);
    generation.usage_activation = meter.live_activation();
    let generation = Arc::new(generation);
    if config.filesystem {
        let source = FilesystemUsageSource::new(Arc::new(GenerationUsageReader {
            reader: generation.allocation_reader.clone(),
        }));
        install_filesystem_usage(&meter, source);
    }
    Ok(AgentFilesystem {
        generation: Some(generation),
        stage: Some(Reconstructing {
            meter,
            initial_files_materialized: false,
            replay_drained: false,
        }),
    })
}

/// Creates a generation handle for filesystem calls made while oplog replay is running.
///
/// Callers request this after initial files are materialized and before replay is drained. It
/// returns `Transitioning` outside that interval and `Revoked` after access has been sealed or
/// invalidated; handles never grant access to another filesystem generation.
pub(crate) fn reconstruction_generation_handle<Adapter: SandboxFilesystemAdapter>(
    filesystem: &ReconstructingFilesystem<Adapter>,
) -> Result<FilesystemGenerationHandle<Adapter>, AccessError> {
    let stage = filesystem
        .stage
        .as_ref()
        .expect("agent filesystem stage already consumed");
    if !stage.initial_files_materialized || stage.replay_drained {
        return Err(AccessError::Transitioning);
    }
    let generation = filesystem
        .generation
        .as_ref()
        .expect("agent filesystem generation already consumed");
    if !generation.registry.replay_access_allowed() {
        return Err(AccessError::Revoked);
    }
    Ok(FilesystemGenerationHandle {
        generation: Arc::downgrade(generation),
        phase: GenerationHandlePhase::Reconstruction,
        invocation: None,
    })
}

/// Creates the generation handle used by a live worker for filesystem calls.
///
/// Callers use this only for a `Resident` filesystem after reconstruction succeeds. The handle is
/// generation-bound and later admissions fail once that generation is sealed or dropped.
pub(crate) fn resident_generation_handle<Adapter: SandboxFilesystemAdapter>(
    filesystem: &ResidentFilesystem<Adapter>,
) -> FilesystemGenerationHandle<Adapter> {
    FilesystemGenerationHandle {
        generation: Arc::downgrade(
            filesystem
                .generation
                .as_ref()
                .expect("agent filesystem generation already consumed"),
        ),
        phase: GenerationHandlePhase::Resident,
        invocation: None,
    }
}

/// Opens a resource-usage metering window for a metered filesystem stage.
///
/// Worker startup and invocation admission call this while the filesystem is `Reconstructing` or
/// `Resident`, using the concurrent-agent permit for the same account. The returned future reports
/// metering setup errors and keeps the permit associated with the window on success.
pub(crate) fn open_resource_usage_window<Stage, Adapter>(
    filesystem: &AgentFilesystem<Stage, Adapter>,
    permit: ConcurrentAgentPermit,
) -> impl Future<Output = Result<ResourceUsageMeteringWindow, MeteringOpenError>> + Send + 'static
where
    Stage: MeteredStage,
    Adapter: SandboxFilesystemAdapter,
{
    open_window(
        filesystem
            .stage
            .as_ref()
            .expect("agent filesystem stage already consumed")
            .meter(),
        permit,
    )
}

/// Ends replay access and waits for all admitted replay filesystem calls to finish.
///
/// Worker startup calls this after instance preparation and before finishing reconstruction. It
/// requires materialized initial files and an undrained `Reconstructing` stage. Lifecycle or
/// admission failure returns a sealed filesystem; success remains `Reconstructing`.
pub(crate) fn finish_replay<Adapter: SandboxFilesystemAdapter>(
    filesystem: ReconstructingFilesystem<Adapter>,
) -> impl Future<Output = Result<ReconstructingFilesystem<Adapter>, ReconstructionFailure<Adapter>>>
+ Send
+ 'static {
    let (generation, stage) = filesystem.into_parts();
    let fenced = if !stage.initial_files_materialized || stage.replay_drained {
        Err(ReconstructionFailure {
            filesystem: sealed_filesystem(generation, stage.meter),
            source: Error::RuntimeInvalidated,
        })
    } else {
        match generation.registry.begin_replay_drain() {
            Ok(()) => Ok((generation, stage)),
            Err(error) => Err(ReconstructionFailure {
                filesystem: sealed_filesystem(generation, stage.meter),
                source: Error::Access(error),
            }),
        }
    };
    let (sender, receiver) = tokio::sync::oneshot::channel();
    spawn_module_task(async move {
        let result = match fenced {
            Ok((generation, mut stage)) => {
                generation.registry.wait_for_calls().await;
                stage.replay_drained = true;
                Ok(AgentFilesystem {
                    generation: Some(generation),
                    stage: Some(stage),
                })
            }
            Err(failure) => Err(failure),
        };
        if let Err(unobserved) = sender.send(result) {
            drop(unobserved);
        }
    });
    async move {
        let result = receiver
            .await
            .expect("module-owned replay drain stopped unexpectedly");
        if let Ok(filesystem) = &result {
            filesystem
                .generation
                .as_ref()
                .expect("reconstructing filesystem generation missing")
                .registry
                .finish_transition();
        }
        result
    }
}

/// Verifies reconstructed usage and advances the filesystem to `Resident`.
///
/// Worker startup calls this after `finish_replay`. The transition blocks new calls and waits for
/// admitted calls, then requires materialized initial files, drained replay, and usage within the
/// installed limits. Any failure returns a sealed filesystem for cleanup.
pub(crate) fn finish_reconstruction<Adapter: SandboxFilesystemAdapter>(
    filesystem: ReconstructingFilesystem<Adapter>,
) -> impl Future<Output = Result<ResidentFilesystem<Adapter>, ReconstructionFailure<Adapter>>>
+ Send
+ 'static {
    let (generation, stage) = filesystem.into_parts();
    let fenced = match generation.begin_transition() {
        Ok(()) => Ok((generation, stage)),
        Err(error) => Err(ReconstructionFailure {
            filesystem: sealed_filesystem(generation, stage.meter),
            source: Error::Access(error),
        }),
    };
    let (sender, receiver) = tokio::sync::oneshot::channel();
    spawn_module_task(async move {
        let result = match fenced {
            Ok((generation, stage)) => complete_reconstruction(generation, stage).await,
            Err(failure) => Err(failure),
        };
        if let Err(unobserved) = sender.send(result) {
            drop(unobserved);
        }
    });
    async move {
        let result = receiver
            .await
            .expect("module-owned reconstruction transition stopped unexpectedly");
        if let Ok(filesystem) = &result {
            filesystem
                .generation
                .as_ref()
                .expect("resident filesystem generation missing")
                .registry
                .finish_transition();
        }
        result
    }
}

async fn complete_reconstruction<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    stage: Reconstructing,
) -> Result<ResidentFilesystem<Adapter>, ReconstructionFailure<Adapter>> {
    generation.registry.wait_for_calls().await;
    let result = if stage.initial_files_materialized && stage.replay_drained {
        generation
            .observe_usage()
            .await
            .and_then(|usage| verify_usage_within_limits(usage, *generation.limits.lock().unwrap()))
    } else {
        Err(Error::RuntimeInvalidated)
    };
    match result {
        Ok(()) => Ok(AgentFilesystem {
            generation: Some(generation),
            stage: Some(Resident { meter: stage.meter }),
        }),
        Err(source) => Err(ReconstructionFailure {
            filesystem: sealed_filesystem(generation, stage.meter),
            source,
        }),
    }
}

/// Stops reconstruction and seals the filesystem without waiting for outstanding handles.
///
/// Startup error paths use this while the filesystem is `Reconstructing`. Metering stops and new
/// calls are revoked; callers can then drain or delete the returned `Sealed` filesystem.
pub(crate) fn abort_reconstruction<Adapter: SandboxFilesystemAdapter>(
    filesystem: ReconstructingFilesystem<Adapter>,
) -> SealedFilesystem<Adapter> {
    let (generation, stage) = filesystem.into_parts();
    sealed_filesystem(generation, stage.meter)
}

/// Replaces the limits of a `Resident` filesystem generation.
///
/// Resource-limit updates call this while the worker is stopped at a lifecycle boundary. It blocks
/// new calls, waits for admitted calls, and installs sandbox byte and object limits. Success returns
/// `Resident` when usage fits or `MustUnload` with a sealed filesystem when it does not; install or
/// transition failures also return a sealed filesystem in `LimitFailure`.
pub(crate) fn set_limits<Adapter: SandboxFilesystemAdapter>(
    filesystem: ResidentFilesystem<Adapter>,
    limits: ResolvedStorageLimits,
) -> impl Future<Output = Result<LimitTransition<Adapter>, LimitFailure<Adapter>>> + Send + 'static
{
    let (generation, stage) = filesystem.into_parts();
    let fenced = match generation.begin_transition() {
        Ok(()) => Ok((generation, stage)),
        Err(error) => Err(LimitFailure {
            filesystem: sealed_filesystem(generation, stage.meter),
            source: Error::Access(error),
        }),
    };
    let (sender, receiver) = tokio::sync::oneshot::channel();
    spawn_module_task(async move {
        let result = match fenced {
            Ok((generation, stage)) => complete_limit_transition(generation, stage, limits).await,
            Err(failure) => Err(failure),
        };
        if let Err(unobserved) = sender.send(result) {
            drop(unobserved);
        }
    });
    async move {
        let result = receiver
            .await
            .expect("module-owned filesystem limit transition stopped unexpectedly");
        if let Ok(LimitTransition::Resident(filesystem)) = &result {
            filesystem
                .generation
                .as_ref()
                .expect("resident filesystem generation missing")
                .registry
                .finish_transition();
        }
        result
    }
}

async fn complete_limit_transition<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    stage: Resident,
    limits: ResolvedStorageLimits,
) -> Result<LimitTransition<Adapter>, LimitFailure<Adapter>> {
    generation.registry.wait_for_calls().await;
    let current_limits = *generation.limits.lock().unwrap();
    let sandbox_limits = match (current_limits, limits) {
        (_, ResolvedStorageLimits::Finite(limits)) => Some(limits),
        (ResolvedStorageLimits::Finite(_), ResolvedStorageLimits::Unlimited) => {
            Some(FilesystemLimits {
                allocated_bytes: u64::MAX - u16::MAX as u64,
                filesystem_objects: u64::MAX,
            })
        }
        (ResolvedStorageLimits::Unlimited, ResolvedStorageLimits::Unlimited) => None,
    };
    let result = match sandbox_limits {
        Some(limits) => generation.install_limits(limits).await.map(Some),
        None => Ok(None),
    };
    match result {
        Ok(installed) => {
            if installed
                .zip(sandbox_limits)
                .is_some_and(|(installed, requested)| installed.limits != requested)
            {
                return Err(LimitFailure {
                    filesystem: sealed_filesystem(generation, stage.meter),
                    source: Error::RuntimeInvalidated,
                });
            }
            *generation.limits.lock().unwrap() = limits;
            if installed.is_some_and(|installed| {
                decide_limit_transition(installed.allocation, installed.limits)
                    == LimitDecision::MustUnload
            }) {
                Ok(LimitTransition::MustUnload(sealed_filesystem(
                    generation,
                    stage.meter,
                )))
            } else {
                Ok(LimitTransition::Resident(AgentFilesystem {
                    generation: Some(generation),
                    stage: Some(Resident { meter: stage.meter }),
                }))
            }
        }
        Err(source) => Err(LimitFailure {
            filesystem: sealed_filesystem(generation, stage.meter),
            source,
        }),
    }
}

pub(super) fn decide_limit_transition(
    allocation: crate::storage_volume::FilesystemAllocation,
    limits: FilesystemLimits,
) -> LimitDecision {
    if allocation.allocated_bytes > limits.allocated_bytes
        || allocation.filesystem_objects > limits.filesystem_objects
    {
        LimitDecision::MustUnload
    } else {
        LimitDecision::Resident
    }
}

/// Converts a `Resident` filesystem into a `Sealed` filesystem.
///
/// Worker unload and error paths call this to stop metering and revoke new calls. Existing calls
/// and open nodes may still drain before deletion.
pub(crate) fn seal<Adapter: SandboxFilesystemAdapter>(
    filesystem: ResidentFilesystem<Adapter>,
) -> SealedFilesystem<Adapter> {
    let (generation, stage) = filesystem.into_parts();
    sealed_filesystem(generation, stage.meter)
}

/// Deletes a `Sealed` filesystem after all calls and open nodes have drained.
///
/// Cleanup paths call this after sealing or a failed transition. The returned future completes only
/// after sandbox deletion is verified; it reports deletion, ownership, or cleanup-task failures.
pub(crate) fn delete<Adapter: SandboxFilesystemAdapter>(
    filesystem: SealedFilesystem<Adapter>,
) -> impl Future<Output = Result<(), DeleteFailure>> + Send + 'static {
    let (generation, stage) = filesystem.into_parts();
    generation.registry.seal();
    drop(stage);
    let (sender, receiver) = tokio::sync::oneshot::channel();
    spawn_cleanup(generation, Some(sender));
    async move {
        receiver.await.unwrap_or_else(|_| {
            Err(DeleteFailure::unverified(
                "observe agent filesystem deletion",
            ))
        })
    }
}

/// Deletes a filesystem that failed before metering was bound.
///
/// Startup rollback calls this with the `Created` filesystem returned by a metering bind failure.
/// It seals admission, waits for all resources to drain, and reports sandbox deletion or cleanup-task
/// failures through `DeleteFailure`.
pub(crate) fn delete_created<Adapter: SandboxFilesystemAdapter>(
    filesystem: CreatedFilesystem<Adapter>,
) -> impl Future<Output = Result<(), DeleteFailure>> + Send + 'static {
    let (generation, _) = filesystem.into_parts();
    generation.registry.seal();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    spawn_cleanup(generation, Some(sender));
    async move {
        receiver.await.unwrap_or_else(|_| {
            Err(DeleteFailure::unverified(
                "observe created agent filesystem deletion",
            ))
        })
    }
}

/// Waits until a `Sealed` filesystem has no admitted calls or open nodes.
///
/// Unload code uses this when it must retain the sandbox filesystem rather than delete it. The
/// function has no error result and does not itself close open nodes or delete sandbox resources.
pub(crate) async fn drain_sealed_filesystem<Adapter: SandboxFilesystemAdapter>(
    filesystem: &SealedFilesystem<Adapter>,
) {
    filesystem
        .generation
        .as_ref()
        .expect("sealed filesystem generation already consumed")
        .registry
        .wait_for_drain()
        .await;
}

pub(super) fn sealed_filesystem<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    meter: ResourceUsageMeter,
) -> SealedFilesystem<Adapter> {
    stop_metering(&meter);
    generation.registry.seal();
    AgentFilesystem {
        generation: Some(generation),
        stage: Some(Sealed { _meter: meter }),
    }
}

fn spawn_cleanup<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    observer: Option<tokio::sync::oneshot::Sender<Result<(), DeleteFailure>>>,
) {
    generation
        .release_metrics
        .get_or_init(Default::default)
        .accept();
    let mut observations = generation
        .release_metrics
        .get_or_init(Default::default)
        .observe_cleanup();
    spawn_module_task(async move {
        generation.registry.wait_for_drain().await;
        let mut sandbox = generation.sandbox.write().await;
        let result = match sandbox.take() {
            Some(adapter) => match Arc::try_unwrap(adapter) {
                Ok(adapter) => match Adapter::delete_and_verify(adapter).await {
                    Ok(()) => {
                        generation
                            .release_metrics
                            .get_or_init(Default::default)
                            .deleted();
                        Ok(())
                    }
                    Err(failure) => {
                        generation
                            .release_metrics
                            .get_or_init(Default::default)
                            .fail(crate::metrics::resource_release::Failure::FilesystemDelete);
                        let (adapter, source) = failure.into_parts();
                        *sandbox = Some(Arc::new(adapter));
                        Err(source)
                    }
                },
                Err(adapter) => {
                    generation
                        .release_metrics
                        .get_or_init(Default::default)
                        .fail(crate::metrics::resource_release::Failure::ExclusiveOwnership);
                    *sandbox = Some(adapter);
                    Err(FilesystemStorageError::verification(
                        "take exclusive ownership for agent filesystem deletion",
                        None,
                    ))
                }
            },
            None => Ok(()),
        };
        drop(sandbox);
        for observation in &mut observations {
            observation.returned();
        }
        let result = result.map_err(|source| DeleteFailure {
            source: Arc::new(source),
            retry: Some(Arc::new(move || {
                let (sender, receiver) = tokio::sync::oneshot::channel();
                spawn_cleanup(Arc::clone(&generation), Some(sender));
                Box::pin(async move {
                    receiver.await.unwrap_or_else(|_| {
                        Err(DeleteFailure::unverified(
                            "observe retried agent filesystem deletion",
                        ))
                    })
                })
            })),
        });
        if let Some(observer) = observer {
            let _ = observer.send(result);
        }
    });
}

pub(super) fn spawn_module_task(task: impl Future<Output = ()> + Send + 'static) {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(task);
    } else {
        std::thread::Builder::new()
            .name("agent-filesystem-cleanup".to_string())
            .spawn(move || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to build agent filesystem cleanup runtime")
                    .block_on(task);
            })
            .expect("failed to start agent filesystem cleanup thread");
    }
}

impl<Reader: crate::sandbox_filesystem::SandboxFilesystemAllocationReader> FilesystemUsageReader
    for GenerationUsageReader<Reader>
{
    fn observe(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<FilesystemUsage, FilesystemStorageError>> + Send + '_>>
    {
        let reader = self.reader.clone();
        Box::pin(async move {
            match reader.read_allocation().await {
                Ok(allocation) => Ok(FilesystemUsage::Authoritative {
                    allocated_bytes: allocation.allocated_bytes,
                    filesystem_objects: allocation.filesystem_objects,
                }),
                Err(error) if error.allocation_is_unsupported() => Ok(FilesystemUsage::Unsupported),
                Err(error) => Err(error),
            }
        })
    }
}

fn verify_usage_within_limits(
    usage: FilesystemUsage,
    limits: ResolvedStorageLimits,
) -> Result<(), Error> {
    match (usage, limits) {
        (FilesystemUsage::Unsupported, ResolvedStorageLimits::Unlimited) => Ok(()),
        (FilesystemUsage::Unsupported, ResolvedStorageLimits::Finite(_)) => {
            Err(Error::RuntimeInvalidated)
        }
        (FilesystemUsage::Authoritative { .. }, ResolvedStorageLimits::Unlimited) => Ok(()),
        (
            FilesystemUsage::Authoritative {
                allocated_bytes,
                filesystem_objects,
            },
            ResolvedStorageLimits::Finite(limits),
        ) if allocated_bytes <= limits.allocated_bytes
            && filesystem_objects <= limits.filesystem_objects =>
        {
            Ok(())
        }
        (FilesystemUsage::Authoritative { .. }, ResolvedStorageLimits::Finite(_)) => {
            Err(Error::RuntimeInvalidated)
        }
    }
}
