// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use super::*;
use golem_common::model::component::ComponentId;
use golem_common::model::entity::{AgentEntity, OwnedAgentEntityId};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::tool::ToolName;
use golem_common::model::{AgentId, OwnedAgentId};
use std::future::pending;
use wasmtime::component::{Accessor, AccessorTask, RuntimeActivityKind};
use wasmtime::{AsContextMut, Config, Engine, Store};

use test_r::test;

fn guest_block(runtime: &Arc<RuntimeStore>, run: RuntimeRunId) {
    let block = std::future::poll_fn(|_| {
        let generation = {
            let state = runtime.owner.state.lock().unwrap();
            state.stores[&runtime.store_id].driver.generation as usize
        };
        runtime.observe(RuntimeObservation::DriverInvalidated {
            run,
            generation,
            reason: RuntimeInvalidation::Poll,
        });
        runtime.observe(RuntimeObservation::DriverBlocked { run, generation });
        Poll::<wasmtime::Result<()>>::Pending
    });
    assert!(runtime.drive(block, None, None).now_or_never().is_none());
}

#[test]
async fn guest_delivery_consumes_an_already_published_block() {
    let runtime = RuntimeStore::new(OwnerSuspension::new());
    let run = runtime_run_ids(1)[0];
    guest_block(&runtime, run);
    let mut delivery = Box::pin(runtime.await_guest_blocked());
    assert!(delivery.as_mut().now_or_never().is_some());
}

#[test]
async fn guest_deliveries_consume_distinct_blocks() {
    let runtime = RuntimeStore::new(OwnerSuspension::new());
    let run = runtime_run_ids(1)[0];
    let mut first = Box::pin(runtime.await_guest_blocked());
    let mut second = Box::pin(runtime.await_guest_blocked());
    assert!(first.as_mut().now_or_never().is_none());
    assert!(second.as_mut().now_or_never().is_none());
    guest_block(&runtime, run);
    assert!(second.as_mut().now_or_never().is_some());
    assert!(first.as_mut().now_or_never().is_none());
    guest_block(&runtime, run);
    assert!(first.as_mut().now_or_never().is_some());
}

#[test]
async fn consumed_guest_block_cannot_release_a_later_delivery() {
    let runtime = RuntimeStore::new(OwnerSuspension::new());
    let run = runtime_run_ids(1)[0];
    let mut first = Box::pin(runtime.await_guest_blocked());
    assert!(first.as_mut().now_or_never().is_none());
    guest_block(&runtime, run);
    assert!(first.as_mut().now_or_never().is_some());
    let mut later = Box::pin(runtime.await_guest_blocked());
    assert!(later.as_mut().now_or_never().is_none());
    guest_block(&runtime, run);
    assert!(later.as_mut().now_or_never().is_some());
}

struct PendingTask;

impl AccessorTask<()> for PendingTask {
    async fn run(self, _: &Accessor<()>) -> wasmtime::Result<()> {
        pending().await
    }
}

fn runtime_ids(count: usize) -> Vec<RuntimeActivityId> {
    let mut config = Config::new();
    config.wasm_component_model_async(true);
    let engine = Engine::new(&config).unwrap();
    let mut store = Store::new(&engine, ());
    let ids = Arc::new(Mutex::new(Vec::new()));
    let observed = ids.clone();
    store
        .as_context_mut()
        .set_runtime_observer(Arc::new(move |event| {
            if let RuntimeObservation::ActivityStarted { activity, .. } = event {
                observed.lock().unwrap().push(activity);
            }
        }))
        .unwrap();
    for _ in 0..count {
        store.as_context_mut().spawn(PendingTask);
    }
    let result = ids.lock().unwrap().clone();
    assert_eq!(result.len(), count);
    result
}

fn runtime_run_ids(count: usize) -> Vec<RuntimeRunId> {
    let mut config = Config::new();
    config
        .wasm_component_model_async(true)
        .concurrency_support(true);
    let engine = Engine::new(&config).unwrap();
    let mut store = Store::new(&engine, ());
    let ids = Arc::new(Mutex::new(Vec::new()));
    let observed = ids.clone();
    store
        .as_context_mut()
        .set_runtime_observer(Arc::new(move |event| {
            if let RuntimeObservation::DriverInvalidated {
                run,
                reason: RuntimeInvalidation::Poll,
                ..
            } = event
            {
                observed.lock().unwrap().push(run);
            }
        }))
        .unwrap();
    for _ in 0..count {
        let mut driver = Box::pin(store.run_concurrent(async |_| ()));
        assert!(poll_once(driver.as_mut()).is_ready());
    }
    let result = ids.lock().unwrap().clone();
    assert_eq!(result.len(), count);
    assert!(result.windows(2).all(|pair| pair[0] < pair[1]));
    result
}

#[test]
fn only_the_newest_guest_import_can_reserve_an_invocation_id() {
    let ids = runtime_ids(3);
    let owner = OwnerSuspension::new();
    let store = RuntimeStore::new(owner);
    let started = |activity, kind| {
        store.observe(RuntimeObservation::ActivityStarted { activity, kind });
    };
    started(ids[0], RuntimeActivityKind::Import);
    assert!(matches!(
        store.guest_import_invocation_id(ids[0]),
        GuestImportInvocationId::Reservable
    ));

    // Other activity kinds do not change the guest's import order.
    started(ids[1], RuntimeActivityKind::Background);
    assert!(matches!(
        store.guest_import_invocation_id(ids[0]),
        GuestImportInvocationId::Reservable
    ));

    let invocation_id = DurableInvocationId::new(OplogIndex::from_u64(1), 7);
    store.record_guest_import_invocation_id(ids[0], invocation_id);
    started(ids[2], RuntimeActivityKind::Import);
    assert!(matches!(
        store.guest_import_invocation_id(ids[0]),
        GuestImportInvocationId::Reserved(id) if id == invocation_id
    ));
    assert!(matches!(
        store.guest_import_invocation_id(ids[1]),
        GuestImportInvocationId::Late
    ));

    store.observe(RuntimeObservation::ActivityFinished { activity: ids[0] });
    assert!(matches!(
        store.guest_import_invocation_id(ids[0]),
        GuestImportInvocationId::Late
    ));
}

pub(crate) fn blocked_timer(
    owner: &Arc<OwnerSuspension>,
    deadline: Instant,
) -> (Arc<RuntimeStore>, RuntimeActivityId, SuspensionWait) {
    let ids = runtime_ids(2);
    let run = runtime_run_ids(1)[0];
    let store = RuntimeStore::new(owner.clone());
    store.observe(RuntimeObservation::ActivityStarted {
        activity: ids[0],
        kind: RuntimeActivityKind::Root,
    });
    store.certify_root(ids[0]);
    let timer = store.timer(ids[0], deadline).unwrap();
    let mut state = owner.state.lock().unwrap();
    let driver = &mut state.stores.get_mut(&store.store_id).unwrap().driver;
    driver.runtime_run = Some(run);
    driver.generation = 1;
    driver.blocked_generation = Some(1);
    drop(state);
    (store, ids[1], timer)
}

pub(crate) fn eligible_now(owner: &OwnerSuspension) -> bool {
    owner.prepare(Instant::now(), Duration::ZERO).is_some()
}

pub(crate) fn commit_now(owner: &OwnerSuspension) -> bool {
    owner
        .prepare(Instant::now(), Duration::ZERO)
        .is_some_and(|attempt| owner.commit(&attempt, Instant::now(), Duration::ZERO))
}

fn entity(start: u64) -> EntityInvocationId {
    EntityInvocationId::new(
        OwnedAgentEntityId {
            owner: OwnedAgentId::new(
                EnvironmentId::new(),
                &AgentId {
                    component_id: ComponentId::new(),
                    agent_id: "owner".to_string(),
                },
            ),
            entity: AgentEntity::Tool(ToolName::try_from("timer").unwrap()),
        },
        OplogIndex::from_u64(start),
    )
    .unwrap()
}

#[test]
fn external_receive_is_passive_only_until_ready_or_cancelled() {
    let owner = OwnerSuspension::new();
    let now = Instant::now();
    let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
    let source = store.source();
    let mut activity = owner.register_external();
    assert!(owner.prepare(now, Duration::ZERO).is_none());
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut receive = Box::pin(activity.receive(&source, rx));
    assert!(poll_once(receive.as_mut()).is_pending());
    let attempt = owner.prepare(now, Duration::ZERO).unwrap();
    tx.send(37).unwrap();
    assert!(!owner.commit(&attempt, now, Duration::ZERO));
    assert!(owner.prepare(now, Duration::ZERO).is_none());
    assert_eq!(poll_once(receive.as_mut()), Poll::Ready(Ok(37)));
    drop(receive);
    assert!(owner.prepare(now, Duration::ZERO).is_none());
    let mut receive = Box::pin(activity.receive(&source, pending::<()>()));
    assert!(poll_once(receive.as_mut()).is_pending());
    assert!(owner.prepare(now, Duration::ZERO).is_some());
    drop(receive);
    assert!(owner.prepare(now, Duration::ZERO).is_none());
    drop(activity);
    assert!(owner.prepare(now, Duration::ZERO).is_some());
}

#[test]
fn external_receive_rejects_buffered_ready_and_wake_during_poll() {
    let owner = OwnerSuspension::new();
    let now = Instant::now();
    let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
    let source = store.source();
    let mut activity = owner.register_external();
    let mut receive = Box::pin(activity.receive(&source, std::future::ready(91)));
    assert_eq!(poll_once(receive.as_mut()), Poll::Ready(91));
    assert!(owner.prepare(now, Duration::ZERO).is_none());
    drop(receive);
    let mut receive = Box::pin(activity.receive(
        &source,
        std::future::poll_fn(|cx| {
            cx.waker().wake_by_ref();
            Poll::<()>::Pending
        }),
    ));
    assert!(poll_once(receive.as_mut()).is_pending());
    assert!(owner.prepare(now, Duration::ZERO).is_none());
}

#[test]
fn external_receive_saved_wake_invalidates_later_polls_and_replacements() {
    let owner = OwnerSuspension::new();
    let now = Instant::now();
    let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
    let source = store.source();
    let mut activity = owner.register_external();
    let saved = std::cell::RefCell::new(None::<Waker>);
    let mut receive = Box::pin(activity.receive(
        &source,
        std::future::poll_fn(|cx| {
            if let Some(waker) = saved.borrow().as_ref() {
                waker.wake_by_ref();
            } else {
                saved.replace(Some(cx.waker().clone()));
            }
            Poll::<()>::Pending
        }),
    ));
    assert!(poll_once(receive.as_mut()).is_pending());
    assert!(owner.prepare(now, Duration::ZERO).is_some());
    assert!(poll_once(receive.as_mut()).is_pending());
    assert!(owner.prepare(now, Duration::ZERO).is_none());
    drop(receive);
    let mut receive = Box::pin(activity.receive(
        &source,
        std::future::poll_fn(|_| {
            saved.borrow().as_ref().unwrap().wake_by_ref();
            Poll::<()>::Pending
        }),
    ));
    assert!(poll_once(receive.as_mut()).is_pending());
    assert!(owner.prepare(now, Duration::ZERO).is_none());
}

#[test]
fn external_receive_repoll_revokes_commit_before_readiness() {
    let owner = OwnerSuspension::new();
    let now = Instant::now();
    let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
    let source = store.source();
    let mut activity = owner.register_external();
    let attempt = std::cell::RefCell::new(None::<Attempt>);
    let mut receive = Box::pin(activity.receive(
        &source,
        std::future::poll_fn(|_| match attempt.borrow().as_ref() {
            None => Poll::Pending,
            Some(attempt) => {
                assert!(!owner.commit(attempt, now, Duration::ZERO));
                assert!(owner.prepare(now, Duration::ZERO).is_none());
                Poll::Ready(53)
            }
        }),
    ));
    assert!(poll_once(receive.as_mut()).is_pending());
    attempt.replace(owner.prepare(now, Duration::ZERO));
    assert!(attempt.borrow().is_some());
    assert_eq!(poll_once(receive.as_mut()), Poll::Ready(53));
}

#[test]
fn external_receive_requires_live_same_owner_source_and_a_real_wait() {
    let owner = OwnerSuspension::new();
    let now = Instant::now();
    let (store, _, timer) = blocked_timer(&owner, now + Duration::from_secs(30));
    let run = owner.state.lock().unwrap().stores[&store.store_id]
        .driver
        .runtime_run
        .unwrap();
    let foreign = RuntimeStore::new(OwnerSuspension::new()).source();
    let mut activity = owner.register_external();
    let mut receive = Box::pin(activity.receive(&foreign, pending::<()>()));
    assert!(poll_once(receive.as_mut()).is_pending());
    assert!(owner.prepare(now, Duration::ZERO).is_none());
    drop(receive);
    let source = store.source();
    let mut receive = Box::pin(activity.receive(&source, pending::<()>()));
    assert!(poll_once(receive.as_mut()).is_pending());
    assert!(owner.prepare(now, Duration::ZERO).is_some());
    drop(timer);
    assert!(owner.prepare(now, Duration::ZERO).is_none());
    let id = *owner.state.lock().unwrap().stores[&store.store_id]
        .activities
        .keys()
        .next()
        .unwrap();
    let _timer = store.timer(id, now + Duration::from_secs(30)).unwrap();
    assert!(owner.prepare(now, Duration::ZERO).is_some());
    owner
        .state
        .lock()
        .unwrap()
        .stores
        .get_mut(&store.store_id)
        .unwrap()
        .driver
        .runtime_run_retired = true;
    assert!(owner.prepare(now, Duration::ZERO).is_none());
    owner
        .state
        .lock()
        .unwrap()
        .stores
        .get_mut(&store.store_id)
        .unwrap()
        .driver
        .runtime_run_retired = false;
    store.observe(RuntimeObservation::DriverInvalidated {
        run,
        generation: 2,
        reason: RuntimeInvalidation::DriverDrop,
    });
    assert!(owner.prepare(now, Duration::ZERO).is_none());
    drop(store);
    let (_replacement, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(60));
    assert!(owner.prepare(now, Duration::ZERO).is_none());
}

#[test]
fn external_receive_retains_unpolled_value_after_suspension_commit() {
    let owner = OwnerSuspension::new();
    let now = Instant::now();
    let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
    let source = store.source();
    let mut activity = owner.register_external();
    let polls = std::sync::atomic::AtomicUsize::new(0);
    let drops = std::sync::atomic::AtomicUsize::new(0);
    struct DropGuard<'a>(&'a std::sync::atomic::AtomicUsize);
    impl Drop for DropGuard<'_> {
        fn drop(&mut self) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
    let guard = DropGuard(&drops);
    let polls_ref = &polls;
    let saved = std::cell::RefCell::new(None::<Waker>);
    let saved_ref = &saved;
    let future = std::future::poll_fn(move |cx| {
        let _ = &guard;
        saved_ref.replace(Some(cx.waker().clone()));
        if polls_ref.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0 {
            Poll::Pending
        } else {
            Poll::Ready(73)
        }
    });
    let mut receive = Box::pin(activity.receive(&source, future));
    assert!(poll_once(receive.as_mut()).is_pending());
    let attempt = owner.prepare(now, Duration::ZERO).unwrap();
    assert!(owner.commit(&attempt, now, Duration::ZERO));
    saved.borrow().as_ref().unwrap().wake_by_ref();
    assert!(poll_once(receive.as_mut()).is_pending());
    assert_eq!(polls.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(drops.load(std::sync::atomic::Ordering::Relaxed), 0);
    drop(receive);
    assert_eq!(drops.load(std::sync::atomic::Ordering::Relaxed), 1);
}

#[test]
fn entity_admission_is_unknown_before_first_task_poll() {
    let owner = OwnerSuspension::new();
    let now = Instant::now();
    let (_store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
    let _participant = owner.register_entity(entity(1));
    assert!(owner.prepare(now, Duration::from_secs(10)).is_none());
}

#[test]
fn neutral_waiter_requires_blocked_entity_and_reverts_on_drop() {
    let owner = OwnerSuspension::new();
    let now = Instant::now();
    let (parent, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
    let waiter = runtime_ids(1)[0];
    parent.observe(RuntimeObservation::ActivityStarted {
        activity: waiter,
        kind: RuntimeActivityKind::Import,
    });
    let invocation = entity(1);
    let participant = owner.register_entity(invocation.clone());
    let neutral = parent.neutral_entity(waiter, &invocation).unwrap();
    assert!(owner.prepare(now, Duration::ZERO).is_none());
    let (child, _, _child_timer) = blocked_timer(&owner, now + Duration::from_secs(60));
    let delegated = child.delegate(&invocation).unwrap();
    assert!(owner.prepare(now, Duration::ZERO).is_some());
    drop(delegated);
    assert!(owner.prepare(now, Duration::ZERO).is_none());
    let _delegated = child.delegate(&invocation).unwrap();
    drop(neutral);
    assert!(owner.prepare(now, Duration::ZERO).is_none());
    let _neutral = parent.neutral_entity(waiter, &invocation).unwrap();
    assert!(owner.prepare(now, Duration::ZERO).is_some());
    drop(participant);
    assert!(owner.prepare(now, Duration::ZERO).is_none());
}

#[test]
fn missing_participant_and_unstarted_delegation_fail_closed() {
    let owner = OwnerSuspension::new();
    let invocation = entity(1);
    let now = Instant::now();
    let (parent, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
    let waiter = runtime_ids(1)[0];
    parent.observe(RuntimeObservation::ActivityStarted {
        activity: waiter,
        kind: RuntimeActivityKind::Import,
    });
    let store = RuntimeStore::new(owner.clone());
    assert!(parent.neutral_entity(waiter, &invocation).is_none());
    let _participant = owner.register_entity(invocation.clone());
    let _neutral = parent.neutral_entity(waiter, &invocation).unwrap();
    let _delegated = store.delegate(&invocation).unwrap();
    assert!(owner.prepare(Instant::now(), Duration::ZERO).is_none());
}

#[test]
fn neutral_runtime_chain_keeps_unknown_and_cancelled_waiters_visible() {
    let owner = OwnerSuspension::new();
    let now = Instant::now();
    let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
    let ids = runtime_ids(3);
    for id in &ids {
        store.observe(RuntimeObservation::ActivityStarted {
            activity: *id,
            kind: RuntimeActivityKind::Background,
        });
    }
    let first = store.neutral_runtime(ids[0], &[ids[1]]).unwrap();
    let _second = store.neutral_runtime(ids[1], &[ids[2]]).unwrap();
    assert!(owner.prepare(now, Duration::ZERO).is_none());
    let _wait = store.promise(ids[2]).unwrap();
    let attempt = owner.prepare(now, Duration::ZERO).unwrap();
    drop(first);
    assert!(!owner.commit(&attempt, now, Duration::ZERO));
    assert!(owner.prepare(now, Duration::ZERO).is_none());
}

#[test]
fn delegated_driver_wake_return_and_drop_revoke_suspension() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct ParentWake {
        owner: Arc<OwnerSuspension>,
        called: AtomicBool,
    }
    impl std::task::Wake for ParentWake {
        fn wake(self: Arc<Self>) {
            assert!(self.owner.state.try_lock().is_ok());
            assert!(self.owner.prepare(Instant::now(), Duration::ZERO).is_none());
            self.called.store(true, Ordering::SeqCst);
        }
    }
    for complete in [false, true] {
        let owner = OwnerSuspension::new();
        let runtime = RuntimeStore::new(owner.clone());
        let invocation = entity(1);
        let _participant = owner.register_entity(invocation.clone());
        let mut config = Config::new();
        config.wasm_component_model_async(true);
        let engine = Engine::new(&config).unwrap();
        let mut store = Store::new(&engine, ());
        store
            .as_context_mut()
            .set_runtime_observer(runtime.clone())
            .unwrap();
        let ready = Arc::new(AtomicBool::new(false));
        let saved = Arc::new(Mutex::new(None));
        let ready_inner = ready.clone();
        let saved_inner = saved.clone();
        let observed = runtime.clone();
        let future = store.run_concurrent(async move |accessor| {
            let id = accessor.runtime_activity().unwrap();
            observed.certify_root(id);
            let _timer = observed
                .timer(id, Instant::now() + Duration::from_secs(60))
                .unwrap();
            std::future::poll_fn(|cx| {
                *saved_inner.lock().unwrap() = Some(cx.waker().clone());
                if ready_inner.load(Ordering::SeqCst) {
                    Poll::Ready(Ok::<(), wasmtime::Error>(()))
                } else {
                    Poll::Pending
                }
            })
            .await
        });
        let mut driver = Box::pin(runtime.drive(future, Some(&invocation), None));
        let parent = Arc::new(ParentWake {
            owner: owner.clone(),
            called: AtomicBool::new(false),
        });
        let parent_waker = Waker::from(parent.clone());
        assert!(
            driver
                .as_mut()
                .poll(&mut Context::from_waker(&parent_waker))
                .is_pending()
        );
        let attempt = owner.prepare(Instant::now(), Duration::ZERO).unwrap();
        let wake = saved.lock().unwrap().take().unwrap();
        std::thread::spawn(move || wake.wake()).join().unwrap();
        assert!(parent.called.load(Ordering::SeqCst));
        assert!(!owner.commit(&attempt, Instant::now(), Duration::ZERO));
        assert!(owner.prepare(Instant::now(), Duration::ZERO).is_none());
        assert!(poll_once(driver.as_mut()).is_pending());
        let attempt = owner.prepare(Instant::now(), Duration::ZERO).unwrap();
        if complete {
            ready.store(true, Ordering::SeqCst);
            assert!(poll_once(driver.as_mut()).is_ready());
        }
        drop(driver);
        assert!(!owner.commit(&attempt, Instant::now(), Duration::ZERO));
        assert!(owner.prepare(Instant::now(), Duration::ZERO).is_none());
    }
}

#[test]
#[test_r::timeout("30s")]
async fn parked_borrowed_fiber_drop_releases_observer_lock() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use wasmtime::component::{Component, Linker};

    struct HostDrop {
        owner: Arc<OwnerSuspension>,
        dropped: Arc<AtomicBool>,
    }
    impl Drop for HostDrop {
        fn drop(&mut self) {
            assert!(self.owner.state.try_lock().is_ok());
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    let owner = OwnerSuspension::new();
    let runtime = RuntimeStore::new(owner.clone());
    let mut config = Config::new();
    config
        .wasm_component_model_async(true)
        .concurrency_support(true);
    let engine = Engine::new(&config).unwrap();
    let component = Component::new(
        &engine,
        r#"
            (component
                (import "park" (func $park))
                (core func $park (canon lower (func $park)))
                (core module $m
                    (import "" "park" (func $park))
                    (func (export "run") call $park))
                (core instance $i (instantiate $m
                    (with "" (instance (export "park" (func $park))))))
                (func (export "run") async (canon lift (core func $i "run"))))
        "#,
    )
    .unwrap();
    let entered = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));
    let mut linker = Linker::<()>::new(&engine);
    let host_owner = owner.clone();
    let host_entered = entered.clone();
    let host_dropped = dropped.clone();
    linker
        .root()
        .func_wrap_async("park", move |store, (): ()| {
            let guard = HostDrop {
                owner: host_owner.clone(),
                dropped: host_dropped.clone(),
            };
            let entered = host_entered.clone();
            Box::new(async move {
                let _guard = guard;
                entered.store(true, Ordering::SeqCst);
                pending::<()>().await;
                let _ = store.data();
                Ok(())
            })
        })
        .unwrap();
    let mut store = Store::new(&engine, ());
    store
        .as_context_mut()
        .set_runtime_observer(runtime.clone())
        .unwrap();
    let instance = linker
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();
    let run = instance
        .get_typed_func::<(), ()>(&mut store, "run")
        .unwrap();
    let mut driver = Box::pin(runtime.drive(run.call_async(&mut store, ()), None, None));
    assert!(poll_once(driver.as_mut()).is_pending());
    assert!(entered.load(Ordering::SeqCst));
    assert!(!dropped.load(Ordering::SeqCst));
    assert!(owner.prepare(Instant::now(), Duration::ZERO).is_none());
    drop(driver);
    assert!(owner.prepare(Instant::now(), Duration::ZERO).is_none());
    drop(store);
    assert!(dropped.load(Ordering::SeqCst));
    assert!(
        owner.state.lock().unwrap().stores[&runtime.store_id]
            .activities
            .is_empty()
    );
}

#[test]
fn neutral_caller_cannot_hide_body_wake_or_cleanup() {
    let owner = OwnerSuspension::new();
    let invocation = entity(101);
    let _participant = owner.register_entity(invocation.clone());
    let parent_runtime = RuntimeStore::new(owner.clone());
    let child_runtime = RuntimeStore::new(owner.clone());
    let mut config = Config::new();
    config.wasm_component_model_async(true);
    let engine = Engine::new(&config).unwrap();
    let mut parent_store = Store::new(&engine, ());
    let mut child_store = Store::new(&engine, ());
    parent_store
        .as_context_mut()
        .set_runtime_observer(parent_runtime.clone())
        .unwrap();
    child_store
        .as_context_mut()
        .set_runtime_observer(child_runtime.clone())
        .unwrap();
    let saved = Arc::new(Mutex::new(None));
    let saved_child = saved.clone();
    let observed = child_runtime.clone();
    let child = child_store.run_concurrent(async move |accessor| {
        let id = accessor.runtime_activity().unwrap();
        observed.certify_root(id);
        let _timer = observed
            .timer(id, Instant::now() + Duration::from_secs(60))
            .unwrap();
        std::future::poll_fn(|cx| {
            *saved_child.lock().unwrap() = Some(cx.waker().clone());
            Poll::<wasmtime::Result<()>>::Pending
        })
        .await
    });
    let observed = parent_runtime.clone();
    let awaited_invocation = invocation.clone();
    let parent = parent_store.run_concurrent(async move |accessor| {
        let id = accessor.runtime_activity().unwrap();
        observed.certify_root(id);
        let _neutral = observed.neutral_entity(id, &awaited_invocation).unwrap();
        pending::<wasmtime::Result<()>>().await
    });
    let mut child = Box::pin(child_runtime.drive(child, Some(&invocation), None));
    let mut parent = Box::pin(parent_runtime.drive(parent, None, None));
    assert!(poll_once(child.as_mut()).is_pending());
    assert!(poll_once(parent.as_mut()).is_pending());
    let attempt = owner
        .prepare(Instant::now(), Duration::ZERO)
        .expect("blocked entity permits suspension");
    saved.lock().unwrap().take().unwrap().wake();
    assert!(!owner.commit(&attempt, Instant::now(), Duration::ZERO));
    assert!(owner.prepare(Instant::now(), Duration::ZERO).is_none());
    assert!(poll_once(child.as_mut()).is_pending());
    let attempt = owner.prepare(Instant::now(), Duration::ZERO).unwrap();
    let cleanup = owner.register_external();
    assert!(!owner.commit(&attempt, Instant::now(), Duration::ZERO));
    assert!(owner.prepare(Instant::now(), Duration::ZERO).is_none());
    drop(cleanup);
    let attempt = owner.prepare(Instant::now(), Duration::ZERO).unwrap();
    drop(child);
    assert!(!owner.commit(&attempt, Instant::now(), Duration::ZERO));
    assert!(owner.prepare(Instant::now(), Duration::ZERO).is_none());
}

fn blocked_promise(owner: &Arc<OwnerSuspension>) -> (Arc<RuntimeStore>, SuspensionWait) {
    let ids = runtime_ids(2);
    let store = RuntimeStore::new(owner.clone());
    store.observe(RuntimeObservation::ActivityStarted {
        activity: ids[0],
        kind: RuntimeActivityKind::Root,
    });
    store.certify_root(ids[0]);
    store.observe(RuntimeObservation::ActivityStarted {
        activity: ids[1],
        kind: RuntimeActivityKind::Import,
    });
    let promise = store.promise(ids[1]).unwrap();
    let run = runtime_run_ids(1)[0];
    let mut state = owner.state.lock().unwrap();
    let driver = &mut state.stores.get_mut(&store.store_id).unwrap().driver;
    driver.runtime_run = Some(run);
    driver.generation = 1;
    driver.blocked_generation = Some(1);
    drop(state);
    (store, promise)
}

fn poll_once<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

#[test]
fn registered_unpolled_rpc_vetoes_an_otherwise_eligible_owner() {
    let owner = OwnerSuspension::new();
    let now = Instant::now();
    let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
    let _rpc = store.rpc_activity(Duration::ZERO, Duration::from_secs(5));
    assert!(owner.prepare(now, Duration::from_secs(10)).is_none());
}

#[test]
fn rpc_remote_phase_freezes_grace_and_durable_recheck_deadline() {
    let owner = OwnerSuspension::new();
    let now = Instant::now();
    let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
    let rpc = store.rpc_activity(Duration::ZERO, Duration::from_secs(5));
    let remote = rpc.revoker();
    let mut task = Box::pin(
        rpc.coordinate(async move { remote.remote_wait(pending::<anyhow::Result<()>>()).await }),
    );
    assert!(poll_once(task.as_mut()).is_pending());
    let attempt = owner
        .prepare(Instant::now(), Duration::from_secs(10))
        .unwrap();
    assert!(matches!(attempt.wakeup, Wakeup::At(_)));
    assert!(owner.commit(&attempt, Instant::now(), Duration::from_secs(10)));
}

#[test]
fn concurrent_rpc_publication_cannot_overwrite_wake_or_revocation() {
    let owner = OwnerSuspension::new();
    for iteration in 0..1000 {
        let rpc = owner.register_rpc(Duration::ZERO, Duration::from_secs(5));
        rpc.shared.poll.lock().unwrap().remote_candidate = true;
        let barrier = std::sync::Barrier::new(3);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                barrier.wait();
                rpc.shared.finish_poll(true, 0);
            });
            scope.spawn(|| {
                barrier.wait();
                if iteration % 2 == 0 {
                    rpc.revoker().revoke();
                } else {
                    let wake = Arc::new(RpcWake {
                        shared: rpc.shared.clone(),
                        forward: futures::task::noop_waker(),
                    });
                    futures::task::ArcWake::wake_by_ref(&wake);
                }
            });
            barrier.wait();
        });
        assert!(matches!(
            owner
                .state
                .lock()
                .unwrap()
                .activities
                .get(&rpc.shared.activity),
            Some(ActivityClass::Unknown)
        ));
    }
}

#[test]
fn retained_completed_remote_wait_cannot_lend_grace_to_a_new_phase() {
    let owner = OwnerSuspension::new();
    let now = Instant::now();
    let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
    let rpc = store.rpc_activity(Duration::from_secs(5), Duration::from_secs(3));
    let remote = rpc.revoker();
    rpc.shared.poll.lock().unwrap().passive_since = Some(now - Duration::from_secs(10));
    let mut completed = Box::pin(remote.remote_wait(std::future::ready(())));
    assert!(poll_once(completed.as_mut()).is_ready());
    let mut next = Box::pin(remote.remote_wait(pending::<()>()));
    assert!(poll_once(next.as_mut()).is_pending());
    rpc.shared.finish_poll(true, 0);
    assert!(
        owner
            .prepare(Instant::now(), Duration::from_secs(10))
            .is_none()
    );
    assert!(
        owner
            .prepare(
                Instant::now() + Duration::from_secs(6),
                Duration::from_secs(10)
            )
            .is_some()
    );
}

#[test]
fn wake_during_rpc_poll_prevents_stale_passive_publication() {
    let owner = OwnerSuspension::new();
    let now = Instant::now();
    let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
    let rpc = store.rpc_activity(Duration::ZERO, Duration::from_secs(5));
    let remote = rpc.revoker();
    let mut task = Box::pin(rpc.coordinate(async move {
        remote
            .remote_wait(std::future::poll_fn(|cx| {
                cx.waker().wake_by_ref();
                Poll::<anyhow::Result<()>>::Pending
            }))
            .await
    }));
    assert!(poll_once(task.as_mut()).is_pending());
    assert!(
        owner
            .prepare(Instant::now(), Duration::from_secs(10))
            .is_none()
    );
}

#[test]
fn remote_repoll_does_not_restart_rpc_suspension_grace() {
    let owner = OwnerSuspension::new();
    let now = Instant::now();
    let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
    let rpc = store.rpc_activity(Duration::from_secs(10), Duration::from_secs(5));
    let shared = rpc.shared.clone();
    let remote = rpc.revoker();
    let captured = Arc::new(Mutex::new(None));
    let task_waker = captured.clone();
    let mut task = Box::pin(rpc.coordinate(async move {
        remote
            .remote_wait(std::future::poll_fn(|cx| {
                *task_waker.lock().unwrap() = Some(cx.waker().clone());
                Poll::<anyhow::Result<()>>::Pending
            }))
            .await
    }));

    assert!(poll_once(task.as_mut()).is_pending());
    let first = shared.poll.lock().unwrap().passive_since.unwrap();
    captured.lock().unwrap().take().unwrap().wake();
    assert!(poll_once(task.as_mut()).is_pending());
    assert_eq!(shared.poll.lock().unwrap().passive_since, Some(first));
}

#[test]
fn revocation_is_unknown_until_the_rpc_task_is_actually_dropped() {
    let owner = OwnerSuspension::new();
    let now = Instant::now();
    let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
    let rpc = store.rpc_activity(Duration::ZERO, Duration::from_secs(5));
    let revoker = rpc.revoker();
    let remote = revoker.clone();
    let mut task = Box::pin(
        rpc.coordinate(async move { remote.remote_wait(pending::<anyhow::Result<()>>()).await }),
    );
    assert!(poll_once(task.as_mut()).is_pending());
    assert!(
        owner
            .prepare(Instant::now(), Duration::from_secs(10))
            .is_some()
    );
    revoker.revoke();
    assert!(
        owner
            .prepare(Instant::now(), Duration::from_secs(10))
            .is_none()
    );
    drop(task);
    assert!(
        owner
            .prepare(Instant::now(), Duration::from_secs(10))
            .is_some()
    );
}

#[test]
fn rpc_commit_rejects_expired_persisted_deadline_and_returns_typed_stop() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    struct Woken(AtomicBool);
    impl std::task::Wake for Woken {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let dropped = Arc::new(AtomicBool::new(false));
    let guard = Dropped(dropped.clone());
    let woken = Arc::new(Woken(AtomicBool::new(false)));
    let waker = Waker::from(woken.clone());
    let owner = OwnerSuspension::new();
    let now = Instant::now();
    let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
    let rpc = store.rpc_activity(Duration::ZERO, Duration::from_secs(5));
    let remote = rpc.revoker();
    let mut task = Box::pin(rpc.coordinate(async move {
        let _guard = guard;
        remote.remote_wait(pending::<anyhow::Result<()>>()).await
    }));
    assert!(
        task.as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );
    let attempt = owner
        .prepare(now + Duration::from_secs(1), Duration::from_secs(10))
        .unwrap();
    let Wakeup::At(deadline) = attempt.wakeup else {
        panic!("RPC requires a persisted wakeup")
    };
    assert!(!owner.commit(&attempt, deadline, Duration::from_secs(10)));
    assert!(!owner.commit(
        &attempt,
        deadline + Duration::from_millis(1),
        Duration::from_secs(10)
    ));
    let fresh = owner.prepare(deadline, Duration::from_secs(10)).unwrap();
    assert!(owner.commit(
        &fresh,
        deadline + Duration::from_secs(1),
        Duration::from_secs(10)
    ));
    assert!(woken.0.load(Ordering::SeqCst));
    let Poll::Ready(Err(error)) = poll_once(task.as_mut()) else {
        panic!("committed stop must complete task")
    };
    assert!(matches!(
        error.downcast_ref::<InterruptKind>(),
        Some(InterruptKind::Suspend(_))
    ));
    assert!(dropped.load(Ordering::SeqCst));
    assert!(owner.state.lock().unwrap().external_activities.is_empty());
}

#[test]
fn cancelled_remote_phase_cannot_certify_local_pending_work() {
    let owner = OwnerSuspension::new();
    let now = Instant::now();
    let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
    let rpc = store.rpc_activity(Duration::ZERO, Duration::from_secs(5));
    let remote = rpc.revoker();
    let mut task = Box::pin(rpc.coordinate(async move {
        {
            let mut wait = Box::pin(remote.remote_wait(pending::<()>()));
            std::future::poll_fn(|cx| {
                assert!(wait.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
        }
        pending::<anyhow::Result<()>>().await
    }));
    assert!(poll_once(task.as_mut()).is_pending());
    assert!(
        owner
            .prepare(Instant::now(), Duration::from_secs(10))
            .is_none()
    );
}

#[test]
fn first_poll_in_an_unstarted_sibling_excludes_suspension() {
    let owner = OwnerSuspension::new();
    let now = Instant::now();
    let (_eligible, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(10));
    let sibling = RuntimeStore::new(owner.clone());
    owner
        .state
        .lock()
        .unwrap()
        .stores
        .get_mut(&sibling.store_id)
        .unwrap()
        .driver
        .active_poll = true;

    assert!(owner.prepare(now, Duration::from_secs(1)).is_none());
}

#[test]
fn earliest_asymmetric_deadline_wins_and_short_deadline_refuses() {
    let owner = OwnerSuspension::new();
    let now = Instant::now();
    let late = now + Duration::from_secs(20);
    let early = now + Duration::from_secs(10);
    let (_first, _, _first_timer) = blocked_timer(&owner, late);
    let (_second, _, _second_timer) = blocked_timer(&owner, early);
    assert_eq!(
        owner.prepare(now, Duration::from_secs(5)).unwrap().wakeup,
        Wakeup::At(early)
    );

    let short_owner = OwnerSuspension::new();
    let (_store, _, _timer) = blocked_timer(&short_owner, now + Duration::from_millis(999));
    assert!(short_owner.prepare(now, Duration::from_secs(1)).is_none());
}

#[test]
fn wake_invalidates_prepare_and_blocked_proof() {
    let owner = OwnerSuspension::new();
    let now = Instant::now();
    let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(10));
    let run = owner.state.lock().unwrap().stores[&store.store_id]
        .driver
        .runtime_run
        .unwrap();
    let attempt = owner.prepare(now, Duration::from_secs(1)).unwrap();
    store.observe(RuntimeObservation::DriverInvalidated {
        run,
        generation: 2,
        reason: RuntimeInvalidation::Wake,
    });
    assert!(
        owner
            .state
            .lock()
            .unwrap()
            .stores
            .get(&store.store_id)
            .unwrap()
            .driver
            .blocked_generation
            .is_none()
    );
    assert!(!owner.commit(&attempt, now, Duration::from_secs(1)));
}

#[test]
fn new_work_or_earlier_deadline_between_prepare_and_commit_refuses() {
    let owner = OwnerSuspension::new();
    let now = Instant::now();
    let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(20));
    let attempt = owner.prepare(now, Duration::from_secs(1)).unwrap();
    let new_activity = runtime_ids(1)[0];
    store.observe(RuntimeObservation::ActivityStarted {
        activity: new_activity,
        kind: RuntimeActivityKind::Import,
    });
    assert!(!owner.commit(&attempt, now, Duration::from_secs(1)));

    store.observe(RuntimeObservation::ActivityFinished {
        activity: new_activity,
    });
    let attempt = owner.prepare(now, Duration::from_secs(1)).unwrap();
    let earlier = runtime_ids(1)[0];
    store.observe(RuntimeObservation::ActivityStarted {
        activity: earlier,
        kind: RuntimeActivityKind::Root,
    });
    store.certify_root(earlier);
    let _earlier_timer = store.timer(earlier, now + Duration::from_secs(10)).unwrap();
    assert!(!owner.commit(&attempt, now, Duration::from_secs(1)));
}

#[test]
fn retired_run_poll_cannot_replace_current_run_and_old_callbacks_are_inert() {
    let owner = OwnerSuspension::new();
    let store = RuntimeStore::new(owner.clone());
    let ids = runtime_run_ids(2);
    {
        let mut state = owner.state.lock().unwrap();
        let driver = &mut state.stores.get_mut(&store.store_id).unwrap().driver;
        driver.runtime_run = Some(ids[1]);
        driver.runtime_watermark = 7;
        driver.runtime_candidate = Some(8);
        driver.active_poll = true;
    }
    let revision = owner.state.lock().unwrap().revision;
    for (run, retired) in [(ids[0], false), (ids[1], true)] {
        owner
            .state
            .lock()
            .unwrap()
            .stores
            .get_mut(&store.store_id)
            .unwrap()
            .driver
            .runtime_run_retired = retired;
        for reason in [
            RuntimeInvalidation::Poll,
            RuntimeInvalidation::Wake,
            RuntimeInvalidation::DriverDrop,
        ] {
            store.observe(RuntimeObservation::DriverInvalidated {
                run,
                generation: 99,
                reason,
            });
        }
        store.observe(RuntimeObservation::DriverBlocked {
            run,
            generation: 100,
        });
    }
    let state = owner.state.lock().unwrap();
    let driver = &state.stores.get(&store.store_id).unwrap().driver;
    assert_eq!(driver.runtime_run, Some(ids[1]));
    assert_eq!(driver.runtime_watermark, 7);
    assert_eq!(driver.runtime_candidate, Some(8));
    assert_eq!(state.revision, revision);
}

#[test]
fn only_a_newer_poll_admits_a_run_and_drop_is_terminal() {
    let owner = OwnerSuspension::new();
    let store = RuntimeStore::new(owner.clone());
    let ids = runtime_run_ids(3);
    for run in ids {
        let revision = owner.state.lock().unwrap().revision;
        for reason in [RuntimeInvalidation::Wake, RuntimeInvalidation::DriverDrop] {
            store.observe(RuntimeObservation::DriverInvalidated {
                run,
                generation: 100,
                reason,
            });
        }
        store.observe(RuntimeObservation::DriverBlocked {
            run,
            generation: 101,
        });
        assert_eq!(owner.state.lock().unwrap().revision, revision);
        store.observe(RuntimeObservation::DriverInvalidated {
            run,
            generation: 2,
            reason: RuntimeInvalidation::Poll,
        });
        {
            let state = owner.state.lock().unwrap();
            let driver = &state.stores[&store.store_id].driver;
            assert_eq!(driver.runtime_run, Some(run));
            assert_eq!(driver.runtime_watermark, 2);
            assert!(!driver.runtime_run_retired);
        }
        // Retirement is terminal even when its generation does not advance.
        store.observe(RuntimeObservation::DriverInvalidated {
            run,
            generation: 2,
            reason: RuntimeInvalidation::DriverDrop,
        });
        let revision = owner.state.lock().unwrap().revision;
        store.observe(RuntimeObservation::DriverInvalidated {
            run,
            generation: 200,
            reason: RuntimeInvalidation::Poll,
        });
        let state = owner.state.lock().unwrap();
        assert!(state.stores[&store.store_id].driver.runtime_run_retired);
        assert_eq!(state.stores[&store.store_id].driver.runtime_watermark, 2);
        assert_eq!(state.revision, revision);
    }
}

#[test]
fn timer_wait_cancellation_invalidates_eligibility() {
    let owner = OwnerSuspension::new();
    let now = Instant::now();
    let (_store, _, timer) = blocked_timer(&owner, now + Duration::from_secs(10));
    assert!(owner.prepare(now, Duration::from_secs(1)).is_some());
    drop(timer);
    assert!(owner.prepare(now, Duration::from_secs(1)).is_none());
}

#[test]
fn promise_only_uses_activation_without_a_timer_deadline() {
    let owner = OwnerSuspension::new();
    let now = Instant::now();
    let (_store, _promise) = blocked_promise(&owner);
    assert_eq!(
        owner.prepare(now, Duration::from_secs(1)).unwrap().wakeup,
        Wakeup::OnActivation
    );
}

#[test]
fn timer_and_promise_use_timer_deadline_and_short_timer_blocks() {
    let owner = OwnerSuspension::new();
    let now = Instant::now();
    let (_promise_store, _promise) = blocked_promise(&owner);
    let deadline = now + Duration::from_secs(10);
    let (_timer_store, _, _timer) = blocked_timer(&owner, deadline);
    assert_eq!(
        owner.prepare(now, Duration::from_secs(1)).unwrap().wakeup,
        Wakeup::At(deadline)
    );

    let short_owner = OwnerSuspension::new();
    let (_promise_store, _promise) = blocked_promise(&short_owner);
    let (_timer_store, _, _timer) = blocked_timer(&short_owner, now + Duration::from_millis(500));
    assert!(short_owner.prepare(now, Duration::from_secs(1)).is_none());
}

#[test]
async fn promise_readiness_and_cancellation_invalidate_classification() {
    let owner = OwnerSuspension::new();
    let now = Instant::now();
    let (_store, promise) = blocked_promise(&owner);
    assert!(owner.prepare(now, Duration::from_secs(1)).is_some());
    promise
        .wait(std::future::ready(()), pending())
        .await
        .unwrap();
    assert!(owner.prepare(now, Duration::from_secs(1)).is_none());

    let owner = OwnerSuspension::new();
    let (_store, promise) = blocked_promise(&owner);
    assert!(owner.prepare(now, Duration::from_secs(1)).is_some());
    drop(promise);
    assert!(owner.prepare(now, Duration::from_secs(1)).is_none());
}

#[test]
fn ready_receive_outlives_its_runtime_activity_but_not_owner_suspension() {
    for replaced in [false, true] {
        let owner = OwnerSuspension::new();
        let (store, activity, _timer) =
            blocked_timer(&owner, Instant::now() + Duration::from_secs(60));
        store.observe(RuntimeObservation::ActivityStarted {
            activity,
            kind: RuntimeActivityKind::Import,
        });
        let wait = store
            .rpc_wait(activity, Duration::ZERO, Duration::from_secs(10))
            .unwrap();
        let _replacement = if replaced {
            Some(store.promise(activity).unwrap())
        } else {
            store.observe(RuntimeObservation::ActivityFinished { activity });
            None
        };
        assert!(eligible_now(&owner));
        let mut result = Box::pin(wait.wait_result(std::future::ready(37), pending()));
        assert!(matches!(poll_once(result.as_mut()), Poll::Ready(Ok(37))));
        assert!(eligible_now(&owner));
    }

    let owner = OwnerSuspension::new();
    let (_store, promise) = blocked_promise(&owner);
    assert!(commit_now(&owner));
    let mut result = Box::pin(promise.wait_result(std::future::ready(37), pending()));
    assert!(matches!(
        poll_once(result.as_mut()),
        Poll::Ready(Err(InterruptKind::Suspend(_)))
    ));
}

#[test]
fn coordinator_generations_are_isolated() {
    let first = OwnerSuspension::new();
    let second = OwnerSuspension::new();
    let now = Instant::now();
    let (_first_store, _, _first_timer) = blocked_timer(&first, now + Duration::from_secs(10));
    let (second_store, _, _second_timer) = blocked_timer(&second, now + Duration::from_secs(10));
    let second_run = second.state.lock().unwrap().stores[&second_store.store_id]
        .driver
        .runtime_run
        .unwrap();
    let attempt = first.prepare(now, Duration::from_secs(1)).unwrap();
    second_store.observe(RuntimeObservation::DriverInvalidated {
        run: second_run,
        generation: 2,
        reason: RuntimeInvalidation::Wake,
    });
    assert!(first.commit(&attempt, now, Duration::from_secs(1)));
    assert!(second.prepare(now, Duration::from_secs(1)).is_none());
}

#[test]
async fn interrupt_wins_over_committed_suspend_and_ready_timer() {
    let owner = OwnerSuspension::new();
    let now = Instant::now();
    let (_store, _, timer) = blocked_timer(&owner, now + Duration::from_secs(10));
    let attempt = owner.prepare(now, Duration::from_secs(1)).unwrap();
    assert!(owner.commit(&attempt, now, Duration::from_secs(1)));
    let interrupted_at = golem_common::model::Timestamp::now_utc();
    let result = timer
        .wait(
            tokio::time::sleep_until(now.into()),
            std::future::ready(InterruptKind::Interrupt(interrupted_at)),
        )
        .await;
    assert!(
        matches!(result, Err(InterruptKind::Interrupt(timestamp)) if timestamp == interrupted_at)
    );
}

#[test]
async fn committed_suspend_wins_over_ready_timer_without_interrupt() {
    let owner = OwnerSuspension::new();
    let now = Instant::now();
    let (_store, _, timer) = blocked_timer(&owner, now + Duration::from_secs(10));
    let attempt = owner.prepare(now, Duration::from_secs(1)).unwrap();
    assert!(owner.commit(&attempt, now, Duration::from_secs(1)));
    let result = timer
        .wait(tokio::time::sleep_until(now.into()), std::future::pending())
        .await;
    assert!(
        matches!(result, Err(InterruptKind::Suspend(timestamp)) if timestamp == attempt.suspend_at)
    );
}
