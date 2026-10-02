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

//! The workers of the clean-ups of `delete_snapshots` and `delete_all_snapshots`. The pending work
//! of each agent is in the state of the registry, and a fixed pool of workers takes the agents
//! whose work is ready. Nothing here can stop the executor: a failure of the store is logged and
//! counted.

use super::registry::Registry;
use super::rules::{self, Work};
use super::store_calls::{Deleted, StoreCalls};
use crate::filesystem_snapshot::AgentSnapshots;
use futures::StreamExt as _;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// Starts `workers` workers of the clean-ups on `jobs`. They end at `shutdown`.
pub(super) fn start(
    calls: &Arc<StoreCalls>,
    registry: &Arc<Registry>,
    shutdown: &CancellationToken,
    jobs: &TaskTracker,
    workers: usize,
) {
    std::iter::repeat_n((), workers).for_each(|()| {
        jobs.spawn(worker(
            Arc::clone(calls),
            Arc::clone(registry),
            shutdown.clone(),
        ));
    });
}

/// Takes the agents whose clean-up is ready and runs their work, one at a time, until the
/// shutdown. The receiver of the wake-ups is taken before each take, so a transition between the
/// take and the wait is not lost.
async fn worker(calls: Arc<StoreCalls>, registry: Arc<Registry>, shutdown: CancellationToken) {
    futures::stream::unfold((), |()| {
        let (calls, registry, shutdown) = (&calls, &registry, &shutdown);
        async move {
            if shutdown.is_cancelled() {
                return None;
            }
            let mut changed = registry.subscribe();
            match registry.apply(rules::take_ready) {
                Some((agent, work)) => {
                    run(calls, registry, &agent, work).await;
                    registry.apply(|state| rules::cleanup_ended(state, &agent));
                    Some(((), ()))
                }
                None => tokio::select! {
                    biased;
                    () = shutdown.cancelled() => None,
                    changed = changed.changed() => changed.ok().map(|()| ((), ())),
                },
            }
        }
    })
    .for_each(|()| std::future::ready(()))
    .await
}

/// Runs the clean-up `work` of `agent`. A delete of names ends when a delete of all snapshots of
/// the agent is requested.
async fn run(calls: &StoreCalls, registry: &Arc<Registry>, agent: &AgentSnapshots, work: Work) {
    let (operation, deleted) = match work {
        Work::Names(names) => {
            let (registry, requested) = (Arc::clone(registry), agent.clone());
            (
                "delete",
                calls
                    .delete(agent, names.into_iter().collect(), async move {
                        registry.until_all_requested(&requested).await
                    })
                    .await,
            )
        }
        Work::All => ("delete_all", calls.delete_all(agent).await),
    };
    if let Deleted::Leaked(error) = deleted {
        tracing::warn!(
            error = %error,
            agent = ?agent,
            operation,
            "Failed to delete filesystem snapshots"
        );
        crate::metrics::filesystem_snapshots::record_leaked_cleanup(operation);
    }
}
