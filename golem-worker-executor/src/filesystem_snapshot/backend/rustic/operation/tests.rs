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

use super::{Operation, Resources};
use crate::filesystem_snapshot::agent_snapshots::AgentSnapshots;
use crate::filesystem_snapshot::backend::rustic::cpu_priority::LowPriority;
use crate::filesystem_snapshot::backend::rustic::repository::RepositoryKey;
use crate::filesystem_snapshot::clock::{Clock, SystemClock};
use crate::filesystem_snapshot::operation_drain::{OperationDrain, OperationLease};
use crate::filesystem_snapshot::test_support::new_scope;
use golem_service_base::storage::blob::BlobStorage;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use std::sync::Arc;
use std::time::Duration;
use test_r::test;
use tokio::runtime::Handle;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

struct Fixture {
    storage: Arc<dyn BlobStorage>,
    root: CancellationToken,
    tracker: TaskTracker,
    operations: OperationDrain,
    read_slots: Arc<Semaphore>,
    agent: AgentSnapshots,
    key: RepositoryKey,
    clock: Arc<dyn Clock>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            storage: Arc::new(InMemoryBlobStorage::new()),
            root: CancellationToken::new(),
            tracker: TaskTracker::new(),
            operations: OperationDrain::default(),
            read_slots: Arc::new(Semaphore::new(1)),
            agent: new_scope(),
            key: RepositoryKey::new([0; 64]),
            clock: Arc::new(SystemClock),
        }
    }

    fn operation<'a>(&'a self, lease: &'a OperationLease) -> Operation<'a> {
        Operation::new(
            lease,
            Resources {
                storage: &self.storage,
                root: &self.root,
                tracker: &self.tracker,
                key: &self.key,
                low_priority: LowPriority::new(None),
                clock: &self.clock,
                deadline: Duration::from_secs(2),
                tries: 1,
                read_slots: &self.read_slots,
            },
        )
    }
}

#[test]
fn construction_needs_no_runtime_and_registers_no_store_task() {
    std::thread::spawn(|| {
        assert!(Handle::try_current().is_err());
        let fixture = Fixture::new();
        let lease = fixture.operations.begin(&fixture.agent);
        let operation = fixture.operation(&lease);

        assert_eq!(fixture.tracker.len(), 0);
        assert!(!operation.root().is_cancelled());
        assert_eq!(operation.agent(), &fixture.agent);

        let tracked = operation.track(std::future::pending::<()>());
        assert_eq!(fixture.tracker.len(), 1);
        drop(tracked);
        assert_eq!(fixture.tracker.len(), 0);

        let other = new_scope();
        let other_lease = fixture.operations.begin(&other);
        let operation = fixture.operation(&other_lease);
        assert_eq!(operation.agent(), &other);
        assert_ne!(operation.agent(), &fixture.agent);
    })
    .join()
    .unwrap();
}

#[test]
async fn the_backend_and_run_guard_retain_their_separate_lifetimes() {
    let fixture = Fixture::new();
    let lease = fixture.operations.begin(&fixture.agent);
    let operation = fixture.operation(&lease);
    let mut run = operation.begin_run().unwrap();
    let token = run.token().clone();
    let backend = run.take_backend();

    assert_eq!(fixture.tracker.len(), 1);
    assert!(!token.is_cancelled());
    drop(run);
    assert!(token.is_cancelled());
    assert!(!fixture.root.is_cancelled());
    assert_eq!(fixture.tracker.len(), 1);
    drop(backend);
    assert_eq!(fixture.tracker.len(), 0);
}

#[test]
async fn an_untransferred_backend_drops_before_its_run_is_cancelled() {
    struct OnCancel {
        tracker: TaskTracker,
        observed: std::sync::atomic::AtomicUsize,
    }
    impl futures::task::ArcWake for OnCancel {
        fn wake_by_ref(this: &Arc<Self>) {
            this.observed
                .store(this.tracker.len(), std::sync::atomic::Ordering::SeqCst);
        }
    }

    let fixture = Fixture::new();
    let lease = fixture.operations.begin(&fixture.agent);
    let operation = fixture.operation(&lease);
    let run = operation.begin_run().unwrap();
    let token = run.token().clone();
    let observer = Arc::new(OnCancel {
        tracker: fixture.tracker.clone(),
        observed: std::sync::atomic::AtomicUsize::new(usize::MAX),
    });
    let waker = futures::task::waker(observer.clone());
    let mut context = std::task::Context::from_waker(&waker);
    let mut cancelled = std::pin::pin!(token.cancelled());
    assert!(cancelled.as_mut().poll(&mut context).is_pending());
    assert_eq!(fixture.tracker.len(), 1);

    drop(run);

    assert!(token.is_cancelled());
    assert_eq!(
        observer.observed.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

#[test]
fn cleanup_checks_shutdown_before_capturing_a_runtime() {
    std::thread::spawn(|| {
        assert!(Handle::try_current().is_err());
        let fixture = Fixture::new();
        let lease = fixture.operations.begin(&fixture.agent);
        let operation = fixture.operation(&lease);
        fixture.root.cancel();

        let error = operation
            .cleanup_tasks()
            .err()
            .expect("shutdown must refuse cleanup creation");

        assert_eq!(
            error.to_string(),
            "the filesystem snapshot store is shut down"
        );
        assert_eq!(fixture.tracker.len(), 0);
    })
    .join()
    .unwrap();
}

#[test]
async fn cleanup_handoff_keeps_store_and_incarnation_registrations_until_the_task_ends() {
    let fixture = Fixture::new();
    let lease = fixture.operations.begin(&fixture.agent);
    let tasks = fixture.operation(&lease).cleanup_tasks().unwrap();
    assert_eq!(fixture.tracker.len(), 1);
    let gate = CancellationToken::new();
    let task = tasks.spawn({
        let gate = gate.clone();
        async move { gate.cancelled().await }
    });
    assert_eq!(fixture.tracker.len(), 2);
    drop(tasks);
    drop(lease);
    assert_eq!(fixture.tracker.len(), 1);
    let mut drained = std::pin::pin!(fixture.operations.drain_before(&fixture.agent));
    assert!(futures::poll!(drained.as_mut()).is_pending());

    gate.cancel();
    task.await.unwrap();
    let _drain_lease = drained.await;

    assert_eq!(fixture.tracker.len(), 0);
}
