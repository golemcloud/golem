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

use super::{BackendRun, Execution};
use crate::filesystem_snapshot::agent_snapshots::AgentSnapshots;
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
        }
    }

    fn execution<'a>(&'a self, lease: &'a OperationLease) -> Execution<'a> {
        Execution::new(
            &self.storage,
            &self.root,
            &self.tracker,
            lease,
            Duration::from_secs(2),
            1,
            &self.read_slots,
        )
    }
}

#[test]
fn construction_needs_no_runtime_and_registers_no_store_task() {
    std::thread::spawn(|| {
        assert!(Handle::try_current().is_err());
        let fixture = Fixture::new();
        let lease = fixture.operations.begin(&fixture.agent);
        let execution = fixture.execution(&lease);

        assert_eq!(fixture.tracker.len(), 0);
        assert!(!execution.root().is_cancelled());

        let tracked = execution.track(std::future::pending::<()>());
        assert_eq!(fixture.tracker.len(), 1);
        drop(tracked);
        assert_eq!(fixture.tracker.len(), 0);
    })
    .join()
    .unwrap();
}

#[test]
async fn the_backend_and_run_guard_retain_their_separate_lifetimes() {
    let fixture = Fixture::new();
    let lease = fixture.operations.begin(&fixture.agent);
    let execution = fixture.execution(&lease);
    let BackendRun {
        token,
        guard,
        backend,
    } = execution.begin_run(&fixture.agent).unwrap();

    assert_eq!(fixture.tracker.len(), 1);
    assert!(!token.is_cancelled());
    drop(guard);
    assert!(token.is_cancelled());
    assert!(!fixture.root.is_cancelled());
    assert_eq!(fixture.tracker.len(), 1);
    drop(backend);
    assert_eq!(fixture.tracker.len(), 0);
}
