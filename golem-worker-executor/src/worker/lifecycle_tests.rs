use super::*;
use test_r::{test, timeout};

#[test]
#[timeout("5s")]
async fn captured_waiting_task_join_stays_pending_until_entered_io_finishes() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut peer = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut backend, _) = listener.accept().unwrap();
    let (started, entered) = tokio::sync::oneshot::channel();
    let task = tokio::task::spawn_blocking(move || {
        started.send(()).unwrap();
        backend.read_exact(&mut [0u8]).unwrap();
    });
    entered.await.unwrap();
    let metrics = crate::metrics::resource_release::tests::TestMetrics::new();
    let scope = metrics.scope();
    let driver = scope.accepted_driver();
    scope.freeze_cause(crate::metrics::resource_release::Cause::Interrupt);
    let waiting = scope.accepted_join();
    let mut joining = Box::pin(join_waiting_task(task, &scope, waiting));
    assert!(futures::poll!(joining.as_mut()).is_pending());
    assert_eq!(
        metrics.count("accepted_stop", "permit_wait_joined", "released"),
        0.0
    );
    peer.write_all(&[1]).unwrap();
    joining.await.unwrap();
    driver.complete();
    scope.seal();
    assert_eq!(
        metrics.count("accepted_stop", "permit_wait_joined", "released"),
        1.0
    );
}

#[test]
#[timeout("5s")]
async fn entity_fence_keeps_attached_store_and_registration_until_the_body_exits() {
    use crate::durable_host::tool::operation::{OwnerFailureWinner, OwnerToolOperations};
    use crate::services::active_agents::EntityFence;
    use golem_common::model::entity::{
        EntityActivation, EntityActivationPolicy, EntityInvocationId, EntityInvocationScope,
        ExecutableTarget, FilesystemCapability, InvocationExecutionMode, OwnedAgentEntityId,
    };
    use golem_common::model::tool::{SecretKeyScope, ToolFilesystemAccess};

    struct StoreData(Arc<AtomicBool>);
    impl Drop for StoreData {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let owner = OwnedAgentId::new(
        golem_common::model::environment::EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "fenced-body".into(),
        },
    );
    let activation = Arc::new(
        EntityActivation::new(
            ExecutableTarget::new(ComponentId::new(), 1_u64.try_into().unwrap()),
            1_u64.try_into().unwrap(),
            EntityActivationPolicy::ToolMiddleware {
                middleware_name: "fenced-body".try_into().unwrap(),
                provision: Default::default(),
                config_keys_readable: Default::default(),
                secret_keys_readable: SecretKeyScope::All,
                secret_keys_revealable: SecretKeyScope::All,
                filesystem_access: ToolFilesystemAccess::Unset,
            },
            FilesystemCapability::Incapable,
        )
        .unwrap(),
    );
    let entity = OwnedAgentEntityId {
        owner: owner.clone(),
        entity: activation.entity(),
    };
    let scope = EntityInvocationScope::new(
        EntityInvocationId::new(entity.clone(), OplogIndex::from_u64(2)).unwrap(),
        OplogIndex::from_u64(1),
        activation,
        Principal::Agent(golem_common::model::agent::AgentPrincipal {
            agent_id: owner.agent_id.clone(),
        }),
        InvocationExecutionMode::Live,
        IdempotencyKey::fresh(),
        false,
        IdempotencyKey::fresh(),
    )
    .unwrap();
    let slot = Arc::new(entity_slot::EntitySlot::new(entity.clone()));
    let registration = slot.register(&scope, Default::default()).unwrap();
    let resources = instance::OwnerRuntimeResources::new(
        Arc::new(AtomicResourceEntry::new(
            u64::MAX,
            usize::MAX,
            usize::MAX,
            u64::MAX,
            2,
        )),
        Arc::new(std::sync::RwLock::new(ExecutionStatus::loading(
            AgentMode::Durable,
        ))),
    );
    registration
        .attach_linear_memory(
            crate::services::linear_memory::LinearMemoryTracker::new_with_metering(
                64,
                64,
                AgentMode::Durable,
                false,
                resources.resource_limits(),
                Arc::new(StdMutex::new(MemoryGrant::inert(64))),
                false,
            ),
        )
        .unwrap();
    let primary_dropped = Arc::new(AtomicBool::new(false));
    let entity_dropped = Arc::new(AtomicBool::new(false));
    let engine = wasmtime::Engine::default();
    let primary = ObservedPrimaryStore::new(&engine, StoreData(primary_dropped.clone()), None);
    let entity_store = ObservedPrimaryStore::new(&engine, StoreData(entity_dropped.clone()), None);
    let operations = OwnerToolOperations::new();
    let entities = StdMutex::new(HashMap::from([(entity.entity, slot.clone())]));
    let accepting = AtomicBool::new(true);
    let generation = AtomicU64::new(0);
    let fence = EntityFence {
        operations: &operations,
        entities: &entities,
        accepting: &accepting,
        generation: &generation,
        resources: &resources,
    };
    let kind = InterruptKind::Suspend(Timestamp::from(80_u64));
    fence.begin(OwnerFailureWinner::Lifecycle(kind)).await;
    assert!(!accepting.load(Ordering::Acquire));
    assert!(registration.cancellation().is_cancelled());
    assert!(
        matches!(operations.selected_owner_failure(), Some(OwnerFailureWinner::Lifecycle(actual)) if actual == kind)
    );
    let mut drain = Box::pin(fence.drain(&owner));
    assert!(futures::poll!(drain.as_mut()).is_pending());
    assert!(entities.try_lock().is_ok());
    assert_eq!(slot.active_invocation_count(), 1);
    assert!(slot.active_invocations()[0].store_attached);
    assert_eq!(slot.charged_linear_memory_bytes(), 64);
    assert!(!entity_dropped.load(Ordering::Acquire));
    assert!(!primary_dropped.load(Ordering::Acquire));
    drop(entity_store);
    assert!(entity_dropped.load(Ordering::Acquire));
    assert!(futures::poll!(drain.as_mut()).is_pending());
    drop(registration);
    drain.await;
    assert_eq!(slot.active_invocation_count(), 0);
    assert!(!primary_dropped.load(Ordering::Acquire));
    drop(primary);
    assert!(primary_dropped.load(Ordering::Acquire));
}

pub(super) fn running_lifecycle(
    interrupts: Arc<Interrupts>,
    start_attempt: Uuid,
    owner_generation_started: bool,
) -> WorkerInstance {
    WorkerInstance::Running(RunningWorker {
        handle: None,
        sender: tokio::sync::mpsc::unbounded_channel().0,
        queue: Default::default(),
        waiting_for_command: Default::default(),
        concurrent_agent_permit_held: Arc::new(AtomicBool::new(true)),
        filesystem_activity: Default::default(),
        filesystem_snapshot_slot: Default::default(),
        unload_request: Default::default(),
        idle_since_millis: Default::default(),
        interrupts,
        resume_replay_pending: Default::default(),
        start_attempt,
        owner_generation_started,
    })
}

#[test]
#[timeout("5s")]
async fn rejected_request_does_not_advance_the_establishment_tail() {
    let interrupts = Interrupts::default();
    let ready = InterruptEstablishment::ready();
    let tail = StdMutex::new(ready.clone());
    let mut guard = interrupts.lock().await;
    let kind = InterruptKind::Suspend(Timestamp::from(10_u64));
    let accepted = register_interrupt_request(
        &mut guard,
        &tail,
        kind,
        false,
        UnloadRequest::ordinary(UnloadReason::Suspend),
    )
    .unwrap();
    assert!(Arc::ptr_eq(&accepted.establishment, &tail.lock().unwrap()));
    assert!(
        register_interrupt_request(
            &mut guard,
            &tail,
            InterruptKind::Restart,
            false,
            UnloadRequest::ordinary(UnloadReason::Restart)
        )
        .is_none()
    );
    assert!(Arc::ptr_eq(&accepted.establishment, &tail.lock().unwrap()));
    assert!(matches!(&*guard, WorkerInterruptState::Pending(pending) if pending.kind == kind));
    let mut predecessor = Box::pin(accepted.establishment.wait_for_predecessor());
    assert!(futures::poll!(predecessor.as_mut()).is_ready());
    assert!(accepted.establishment.predecessor.lock().unwrap().is_none());
}

#[test]
#[timeout("5s")]
async fn replacement_keeps_the_preceding_establishment_owned() {
    let interrupts = Interrupts::default();
    let tail = StdMutex::new(InterruptEstablishment::ready());
    let mut guard = interrupts.lock().await;
    let first = register_interrupt_request(
        &mut guard,
        &tail,
        InterruptKind::Restart,
        true,
        UnloadRequest::ordinary(UnloadReason::Restart),
    )
    .unwrap();
    let kind = InterruptKind::Suspend(Timestamp::from(11_u64));
    let last = register_interrupt_request(
        &mut guard,
        &tail,
        kind,
        false,
        UnloadRequest::ordinary(UnloadReason::Suspend),
    )
    .unwrap();
    let request = guard.take().unwrap();
    assert_eq!(request.kind, kind);
    assert!(Arc::ptr_eq(&request.establishment, &last.establishment));
    drop(guard);
    let mut waiting = Box::pin(last.establishment.wait_for_predecessor());
    assert!(futures::poll!(waiting.as_mut()).is_pending());
    first.establishment.complete();
    waiting.await;
    assert!(last.establishment.predecessor.lock().unwrap().is_none());
    assert!(!last.establishment.complete.load(Ordering::Acquire));
    last.establishment.complete();
    last.establishment.wait().await;
}
