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
use crate::durable_host::io::poll::wait_tcp_connect;
use crate::durable_host::io::streams::interruptible_tcp_input;
use crate::durable_host::tool::operation::OwnerToolOperations;
use crate::model::ExecutionStatus;
use golem_common::model::Timestamp;
use golem_common::model::agent::AgentMode;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use test_r::test;
use tokio_util::sync::CancellationToken;
use wasmtime::component::{Resource, ResourceTable};
use wasmtime_wasi::StreamError;
use wasmtime_wasi::p2::bindings::io::poll::HostPollable;
use wasmtime_wasi::p2::bindings::io::streams::{
    HostInputStream, HostOutputStream, InputStream, OutputStream,
};
use wasmtime_wasi::p2::pipe::{AsyncReadStream, MemoryInputPipe, MemoryOutputPipe};

struct Sources {
    status: RwLock<ExecutionStatus>,
    signal: Arc<tokio::sync::broadcast::Sender<InterruptKind>>,
    invocation_deadline: AtomicBool,
    tail_deadline: AtomicBool,
    owner: Arc<OwnerToolOperations>,
    cancellation: CancellationToken,
}

impl Sources {
    fn running() -> Self {
        Self::new(AgentMode::Durable, false)
    }

    fn new(mode: AgentMode, loading: bool) -> Self {
        let signal = Arc::new(tokio::sync::broadcast::channel(1).0);
        Self {
            status: RwLock::new(if loading {
                ExecutionStatus::Loading {
                    agent_mode: mode,
                    timestamp: Timestamp::now_utc(),
                    interrupt_signal: signal.clone(),
                }
            } else {
                ExecutionStatus::Running {
                    agent_mode: mode,
                    timestamp: Timestamp::now_utc(),
                    interrupt_signal: signal.clone(),
                }
            }),
            signal,
            invocation_deadline: AtomicBool::new(false),
            tail_deadline: AtomicBool::new(false),
            owner: OwnerToolOperations::new(),
            cancellation: CancellationToken::new(),
        }
    }

    fn borrow(&self) -> InterruptSources<'_> {
        InterruptSources::new(
            &self.status,
            &self.invocation_deadline,
            &self.tail_deadline,
            self.owner.clone(),
            Some(self.cancellation.clone()),
        )
    }

    fn trigger(&self, source: Source) -> Option<InterruptKind> {
        let kind = InterruptKind::Interrupt(Timestamp::now_utc());
        match source {
            Source::InvocationDeadline => {
                self.invocation_deadline.store(true, Ordering::Release);
                let _ = self.signal.send(kind);
                Some(kind)
            }
            Source::TailDeadline => {
                self.tail_deadline.store(true, Ordering::Release);
                let _ = self.signal.send(kind);
                Some(kind)
            }
            Source::EntityCancellation => {
                self.cancellation.cancel();
                None
            }
            Source::Lifecycle => {
                let kind = InterruptKind::Suspend(Timestamp::now_utc());
                assert!(self.status.write().unwrap().publish_interrupt(
                    kind,
                    Arc::new(tokio::sync::broadcast::channel(1).0),
                    Some(&self.signal),
                ));
                Some(kind)
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Source {
    InvocationDeadline,
    TailDeadline,
    EntityCancellation,
    Lifecycle,
}

#[derive(Clone, Copy)]
enum Order {
    BeforeSubscription,
    DuringWait,
    CompletionFirst,
}

#[derive(Default)]
struct ReadinessObserved {
    pending: AtomicUsize,
    completed: AtomicUsize,
    dropped: AtomicUsize,
}

struct ControlledReadiness {
    ready: tokio::sync::oneshot::Receiver<()>,
    observed: Arc<ReadinessObserved>,
}

#[async_trait::async_trait]
impl wasmtime_wasi::Pollable for ControlledReadiness {
    async fn ready(&mut self) {
        struct Read(Arc<ReadinessObserved>);
        impl Drop for Read {
            fn drop(&mut self) {
                self.0.dropped.fetch_add(1, Ordering::SeqCst);
            }
        }
        let _read = Read(self.observed.clone());
        std::future::poll_fn(|cx| {
            let result = Pin::new(&mut self.ready).poll(cx);
            if result.is_pending() {
                self.observed.pending.fetch_add(1, Ordering::SeqCst);
            }
            result
        })
        .await
        .unwrap();
        self.observed.completed.fetch_add(1, Ordering::SeqCst);
    }
}

struct PendingReadiness;

#[async_trait::async_trait]
impl wasmtime_wasi::Pollable for PendingReadiness {
    async fn ready(&mut self) {
        std::future::pending::<()>().await;
    }
}

fn pending_pollable() -> (wasmtime_wasi::IoCtx, ResourceTable, u32) {
    let (_, io_ctx, mut table) = crate::wasi_host::create_context(
        &[] as &[&str],
        std::io::empty(),
        std::io::empty(),
        std::io::empty(),
        |duration| wasmtime::Error::from(crate::durable_host::SuspendForSleep(duration)),
        None,
        true,
    )
    .unwrap();
    let parent = table.push(PendingReadiness).unwrap();
    let pollable = wasmtime_wasi::subscribe(&mut table, parent, None).unwrap();
    (io_ctx, table, pollable.rep())
}

fn assert_interrupt(result: wasmtime::Result<()>) {
    let error = result.expect_err("native connect wait must trap");
    assert!(
        matches!(
            error.downcast_ref::<InterruptKind>(),
            Some(InterruptKind::Interrupt(_))
        ),
        "expected typed InterruptKind::Interrupt, got {error:?}"
    );
}

#[test]
async fn native_connect_observes_deadline_before_subscription() {
    let sources = Sources::running();
    sources.invocation_deadline.store(true, Ordering::Release);
    assert!(
        sources
            .signal
            .send(InterruptKind::Interrupt(Timestamp::now_utc()))
            .is_err()
    );
    let (mut io_ctx, mut table, rep) = pending_pollable();
    let mut io = wasmtime_wasi::IoData {
        table: &mut table,
        io_ctx: &mut io_ctx,
    };
    let wait = wait_tcp_connect(
        NativeIoWait::new(sources.borrow()),
        HostPollable::block(&mut io, Resource::new_borrow(rep)),
    );
    tokio::pin!(wait);
    let std::task::Poll::Ready(result) = futures::poll!(wait.as_mut()) else {
        panic!("latched deadline was missed by the native connect subscription");
    };
    assert_interrupt(result);
    assert!(sources.status.read().unwrap().is_running());
}

#[test]
async fn native_connect_preserves_lifecycle_kind_over_entity_cancellation() {
    let sources = Sources::running();
    sources.cancellation.cancel();
    let expected = sources.trigger(Source::Lifecycle).unwrap();
    let (mut io_ctx, mut table, rep) = pending_pollable();
    let mut io = wasmtime_wasi::IoData {
        table: &mut table,
        io_ctx: &mut io_ctx,
    };
    let mut wait = Box::pin(wait_tcp_connect(
        NativeIoWait::new(sources.borrow()),
        HostPollable::block(&mut io, Resource::new_borrow(rep)),
    ));
    let std::task::Poll::Ready(result) = futures::poll!(wait.as_mut()) else {
        panic!("published lifecycle stop must be ready");
    };
    assert_eq!(
        result.unwrap_err().downcast_ref::<InterruptKind>(),
        Some(&expected)
    );
}

async fn check_connect(source: Source, order: Order) {
    for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
        for loading in [false, true] {
            let sources = Sources::new(mode, loading);
            let (_, mut io_ctx, mut table) = crate::wasi_host::create_context(
                &[] as &[&str],
                std::io::empty(),
                std::io::empty(),
                std::io::empty(),
                |duration| wasmtime::Error::from(crate::durable_host::SuspendForSleep(duration)),
                None,
                true,
            )
            .unwrap();
            let observed = Arc::new(ReadinessObserved::default());
            let (ready, receive) = tokio::sync::oneshot::channel();
            let native = table
                .push(ControlledReadiness {
                    ready: receive,
                    observed: observed.clone(),
                })
                .unwrap();
            let native_rep = native.rep();
            let pollable = wasmtime_wasi::subscribe(
                &mut table,
                Resource::<ControlledReadiness>::new_borrow(native_rep),
                None,
            )
            .unwrap();
            let borrowed = sources.borrow();
            let mut expected = None;
            if matches!(order, Order::BeforeSubscription) {
                assert_eq!(sources.signal.receiver_count(), 0);
                expected = sources.trigger(source);
            }
            let mut io = wasmtime_wasi::IoData {
                table: &mut table,
                io_ctx: &mut io_ctx,
            };
            let mut wait = Box::pin(wait_tcp_connect(
                NativeIoWait::new(borrowed),
                HostPollable::block(&mut io, Resource::new_borrow(pollable.rep())),
            ));
            let result = match order {
                Order::BeforeSubscription => {
                    let std::task::Poll::Ready(result) = futures::poll!(wait.as_mut()) else {
                        panic!("native connect missed cancellation before subscription");
                    };
                    result
                }
                Order::DuringWait | Order::CompletionFirst => {
                    assert!(futures::poll!(wait.as_mut()).is_pending());
                    assert!(observed.pending.load(Ordering::SeqCst) > 0);
                    assert_eq!(observed.completed.load(Ordering::SeqCst), 0);
                    assert_eq!(observed.dropped.load(Ordering::SeqCst), 0);
                    if matches!(order, Order::CompletionFirst) {
                        ready.send(()).unwrap();
                    } else {
                        expected = sources.trigger(source);
                    }
                    let std::task::Poll::Ready(result) = futures::poll!(wait.as_mut()) else {
                        panic!("native connect did not observe its selected event");
                    };
                    result
                }
            };
            drop(wait);
            if matches!(order, Order::CompletionFirst) {
                result.unwrap();
                assert_eq!(observed.completed.load(Ordering::SeqCst), 1);
                sources.trigger(source);
            } else {
                let error = result.unwrap_err();
                if matches!(order, Order::DuringWait) || matches!(source, Source::Lifecycle) {
                    if let Some(expected) = expected {
                        assert_eq!(error.downcast_ref::<InterruptKind>(), Some(&expected));
                    } else {
                        assert_interrupt(Err(error));
                    }
                } else {
                    assert_interrupt(Err(error));
                }
                assert_eq!(observed.completed.load(Ordering::SeqCst), 0);
            }
            if !matches!(source, Source::Lifecycle) {
                assert!(matches!(
                    &*sources.status.read().unwrap(),
                    ExecutionStatus::Running { .. } | ExecutionStatus::Loading { .. }
                ));
            }
            if observed.pending.load(Ordering::SeqCst) > 0 {
                assert_eq!(observed.dropped.load(Ordering::SeqCst), 1);
            }
            assert!(matches!(
                io.table
                    .delete(Resource::<ControlledReadiness>::new_own(native_rep)),
                Err(wasmtime::component::ResourceTableError::HasChildren)
            ));
            HostPollable::drop(&mut io, Resource::new_own(pollable.rep())).unwrap();
            table
                .delete(Resource::<ControlledReadiness>::new_own(native_rep))
                .unwrap();
        }
    }
}

#[test]
async fn native_connect_observes_entity_cancellation_before_subscription() {
    check_connect(Source::EntityCancellation, Order::BeforeSubscription).await;
}

#[test]
async fn native_connect_interrupts_pending_entity_cancellation() {
    check_connect(Source::EntityCancellation, Order::DuringWait).await;
}

#[test]
async fn native_connect_interrupts_pending_deadline() {
    check_connect(Source::InvocationDeadline, Order::DuringWait).await;
}

#[test]
async fn native_connect_observes_tail_deadline_before_subscription() {
    check_connect(Source::TailDeadline, Order::BeforeSubscription).await;
}

#[test]
async fn native_connect_observes_lifecycle_stop_before_subscription() {
    check_connect(Source::Lifecycle, Order::BeforeSubscription).await;
}

#[test]
async fn native_connect_interrupts_actual_native_pending() {
    check_connect(Source::Lifecycle, Order::DuringWait).await;
}

#[test]
async fn native_connect_completion_precedes_later_stop() {
    for source in [
        Source::InvocationDeadline,
        Source::TailDeadline,
        Source::EntityCancellation,
        Source::Lifecycle,
    ] {
        check_connect(source, Order::CompletionFirst).await;
    }
}

async fn observe_input(
    table: &mut ResourceTable,
    rep: u32,
    read: bool,
    wait: NativeIoWait,
) -> Result<u64, StreamError> {
    if read {
        let native = HostInputStream::blocking_read(table, Resource::new_borrow(rep), 3);
        interruptible_tcp_input(native, wait)
            .await
            .map(|bytes| bytes.len() as u64)
    } else {
        let native = HostInputStream::blocking_skip(table, Resource::new_borrow(rep), 3);
        interruptible_tcp_input(native, wait).await
    }
}

async fn check_input(source: Source, order: Order) {
    for read in [false, true] {
        let sources = Sources::running();
        let (reader, peer) = tokio::io::duplex(8);
        let mut table = ResourceTable::new();
        let input: InputStream = Box::new(AsyncReadStream::new(reader));
        let input = table.push(input).unwrap();
        let borrowed = sources.borrow();
        let mut expected = None;
        if matches!(order, Order::BeforeSubscription) {
            assert_eq!(sources.signal.receiver_count(), 0);
            expected = sources.trigger(source);
        }
        let mut wait = Box::pin(observe_input(
            &mut table,
            input.rep(),
            read,
            NativeIoWait::new(borrowed),
        ));
        if matches!(order, Order::DuringWait) {
            assert!(futures::poll!(wait.as_mut()).is_pending());
            expected = sources.trigger(source);
        }
        let std::task::Poll::Ready(result) = futures::poll!(wait.as_mut()) else {
            panic!("native input missed cancellation");
        };
        drop(wait);
        let StreamError::Trap(error) = result.unwrap_err() else {
            panic!("raw input must return a typed interruption trap");
        };
        if matches!(order, Order::DuringWait) || matches!(source, Source::Lifecycle) {
            if let Some(expected) = expected {
                assert_eq!(error.downcast_ref::<InterruptKind>(), Some(&expected));
            } else {
                assert_interrupt(Err(error));
            }
        } else {
            assert_interrupt(Err(error));
        }
        if !matches!(source, Source::Lifecycle) {
            assert!(sources.status.read().unwrap().is_running());
        }
        HostInputStream::drop(&mut table, Resource::new_own(input.rep()))
            .await
            .unwrap();
        assert!(matches!(
            table.get(&Resource::<InputStream>::new_borrow(input.rep())),
            Err(wasmtime::component::ResourceTableError::NotPresent)
        ));
        drop(peer);
    }
}

#[test]
async fn native_raw_input_observes_deadline_before_subscription() {
    check_input(Source::InvocationDeadline, Order::BeforeSubscription).await;
}

#[test]
async fn native_raw_input_observes_entity_cancellation_before_subscription() {
    check_input(Source::EntityCancellation, Order::BeforeSubscription).await;
}

#[test]
async fn native_raw_input_interrupts_pending_deadline() {
    check_input(Source::InvocationDeadline, Order::DuringWait).await;
}

#[test]
async fn native_raw_input_interrupts_pending_entity_cancellation() {
    check_input(Source::EntityCancellation, Order::DuringWait).await;
}

async fn check_splice(source: Source, order: Order) {
    let sources = Sources::running();
    let (reader, peer) = tokio::io::duplex(8);
    let input: InputStream = if matches!(order, Order::CompletionFirst) {
        Box::new(MemoryInputPipe::new("x"))
    } else {
        Box::new(AsyncReadStream::new(reader))
    };
    let output_pipe = MemoryOutputPipe::new(8);
    let output: OutputStream = Box::new(output_pipe.clone());
    let mut table = ResourceTable::new();
    let input = table.push(input).unwrap();
    let output = table.push(output).unwrap();
    assert_ne!(input.rep(), output.rep());
    assert!(
        HostOutputStream::check_write(&mut table, Resource::new_borrow(output.rep()))
            .await
            .unwrap()
            >= 1,
        "Pending must be caused by input, not output backpressure"
    );
    let borrowed = sources.borrow();
    let mut expected = None;
    if matches!(order, Order::BeforeSubscription) {
        assert_eq!(sources.signal.receiver_count(), 0);
        expected = sources.trigger(source);
    }
    let native = HostOutputStream::blocking_splice(
        &mut table,
        Resource::new_borrow(output.rep()),
        Resource::new_borrow(input.rep()),
        1,
    );
    let mut wait = Box::pin(interruptible_tcp_input(native, NativeIoWait::new(borrowed)));
    let result = match order {
        Order::BeforeSubscription => {
            let std::task::Poll::Ready(result) = futures::poll!(wait.as_mut()) else {
                panic!("native splice missed cancellation before subscription");
            };
            result
        }
        Order::DuringWait => {
            assert!(futures::poll!(wait.as_mut()).is_pending());
            assert!(output_pipe.contents().is_empty());
            expected = sources.trigger(source);
            let std::task::Poll::Ready(result) = futures::poll!(wait.as_mut()) else {
                panic!("native splice missed cancellation during actual Pending");
            };
            result
        }
        Order::CompletionFirst => wait.as_mut().await,
    };
    drop(wait);
    if matches!(order, Order::CompletionFirst) {
        assert_eq!(result.unwrap(), 1);
        assert_eq!(output_pipe.contents().as_ref(), b"x");
        assert_eq!(sources.signal.receiver_count(), 0);
        sources.trigger(source);
        assert_eq!(output_pipe.contents().as_ref(), b"x");
    } else {
        let StreamError::Trap(error) = result.unwrap_err() else {
            panic!("raw splice must return a typed interruption trap");
        };
        if let Some(expected) = expected
            && (matches!(order, Order::DuringWait) || matches!(source, Source::Lifecycle))
        {
            assert_eq!(error.downcast_ref::<InterruptKind>(), Some(&expected));
        } else {
            assert_interrupt(Err(error));
        }
        assert!(output_pipe.contents().is_empty());
    }
    HostInputStream::drop(&mut table, Resource::new_own(input.rep()))
        .await
        .unwrap();
    HostOutputStream::drop(&mut table, Resource::new_own(output.rep()))
        .await
        .unwrap();
    assert!(matches!(
        table.get(&Resource::<InputStream>::new_borrow(input.rep())),
        Err(wasmtime::component::ResourceTableError::NotPresent)
    ));
    assert!(matches!(
        table.get(&Resource::<OutputStream>::new_borrow(output.rep())),
        Err(wasmtime::component::ResourceTableError::NotPresent)
    ));
    drop(peer);
}

#[test]
async fn native_splice_observes_cancellation_before_subscription() {
    for source in [
        Source::InvocationDeadline,
        Source::TailDeadline,
        Source::EntityCancellation,
        Source::Lifecycle,
    ] {
        check_splice(source, Order::BeforeSubscription).await;
    }
}

#[test]
async fn native_splice_interrupts_actual_pending_without_output_effect() {
    for source in [
        Source::InvocationDeadline,
        Source::TailDeadline,
        Source::EntityCancellation,
        Source::Lifecycle,
    ] {
        check_splice(source, Order::DuringWait).await;
    }
}

#[test]
async fn native_splice_completion_precedes_later_cancellation() {
    for source in [
        Source::InvocationDeadline,
        Source::TailDeadline,
        Source::EntityCancellation,
        Source::Lifecycle,
    ] {
        check_splice(source, Order::CompletionFirst).await;
    }
}

#[test]
async fn native_raw_input_completion_precedes_later_cancellation() {
    for read in [false, true] {
        for source in [
            Source::InvocationDeadline,
            Source::EntityCancellation,
            Source::Lifecycle,
        ] {
            let sources = Sources::running();
            let mut table = ResourceTable::new();
            let input: InputStream = Box::new(MemoryInputPipe::new("abc"));
            let input = table.push(input).unwrap();
            let result = observe_input(
                &mut table,
                input.rep(),
                read,
                NativeIoWait::new(sources.borrow()),
            )
            .await;
            assert_eq!(result.unwrap(), 3);
            assert_eq!(sources.signal.receiver_count(), 0);
            sources.trigger(source);
            HostInputStream::drop(&mut table, Resource::new_own(input.rep()))
                .await
                .unwrap();
        }
    }
}
