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

//! The filesystem snapshots of a fork attempt: the hold of the source, the copy into the stage of
//! the attempt, the check of the baseline, and the publication of the target, which decides what
//! happens to the snapshots of the stage.

use super::decisions::{self, ForkFound, ForkOutcome};
use super::transitions::{self, Flight, ForkEnd};
use super::{Core, store_name};
use crate::filesystem_snapshot::{AgentSnapshots, CallError, ReadError};
use crate::services::oplog::StagePublication;
use futures::future::BoxFuture;
use golem_common::model::oplog::FilesystemSnapshotName;
use golem_common::model::{AgentFingerprint, AgentId, OwnedAgentId};
use std::sync::Arc;
use uuid::Uuid;

/// The key that makes a [`StagePublication`]. Only this module makes one, so each publication of
/// a fork goes through [`Copied::publish`].
pub(crate) struct PublicationKey(());

/// Why a fork attempt got no hold of its source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ForkStopped;

impl std::fmt::Display for ForkStopped {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("the filesystem snapshot service stopped before the fork began")
    }
}

/// What the fork found after it tried to publish its target.
pub(crate) enum PublishFound<T, E> {
    /// The publication gave `true`: the stage of this attempt is the target.
    Published(T),
    /// The fork read the `Create` of the live target, with this instance id.
    Live(AgentFingerprint, T),
    /// The publication gave `false`, in the one call that the permission allows.
    Refused(T),
    /// The outcome is not known.
    Unknown(E),
}

/// Whether the target of a fork holds the filesystem snapshot of its baseline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Baseline {
    /// The copy holds the snapshot of the baseline.
    Present,
    /// The copy does not hold the snapshot of the baseline with this name.
    Missing(FilesystemSnapshotName),
    /// The baseline has no filesystem snapshot.
    NotChecked,
}

/// The end of a fork attempt: its hold of the source, the snapshots of its stage once its copy
/// began, and the outcome of its publication once it is known. Dropped, it applies the end of
/// the attempt.
struct ForkTicket {
    core: Arc<Core>,
    from: AgentSnapshots,
    flight: Flight,
    stage: Option<AgentSnapshots>,
    outcome: Option<ForkOutcome>,
}

impl Drop for ForkTicket {
    fn drop(&mut self) {
        let end = self.core.registry.apply(|state| {
            transitions::fork_ended(
                state,
                &self.from,
                &self.flight,
                self.stage.as_ref(),
                self.outcome,
            )
        });
        match end {
            ForkEnd::StageDeleted {
                overflow: Some(transitions::Overflow::Refused),
            } => {
                tracing::warn!(
                    stage = ?self.stage,
                    "The clean-up queue of filesystem snapshots is full; the snapshots of a fork stage are lost"
                );
                crate::metrics::filesystem_snapshots::record_leaked_cleanup("overflow");
            }
            ForkEnd::StageLeaked => {
                tracing::warn!(
                    stage = ?self.stage,
                    "The publication of a fork ended with an unknown outcome; the snapshots of its stage stay"
                );
                crate::metrics::filesystem_snapshots::record_leaked_cleanup("fork_stage");
            }
            ForkEnd::StageDeleted {
                overflow: Some(transitions::Overflow::Evicted(evicted)),
            } => super::record_eviction(&evicted),
            ForkEnd::StageDeleted { overflow: None } | ForkEnd::Done => {}
        }
    }
}

/// The hold of a fork attempt on the snapshots of its source. While it lives, a delete of the
/// snapshots of the source waits, and another attempt of the same fork request waits too.
pub(crate) struct ForkCopy {
    ticket: Option<ForkTicket>,
    waited: bool,
}

impl ForkCopy {
    /// Whether the attempt waited for another attempt of the same request, or for a delete of
    /// the snapshots of the source. Such an attempt reconciles again before it copies.
    pub(crate) fn waited(&self) -> bool {
        self.waited
    }

    /// Copies every snapshot of the source into the stage `stage_id` of `target`, which holds
    /// none, and checks that the snapshot `baseline` resolves there. A copy that fails, or an
    /// attempt that never reaches its publication, has the snapshots of the stage deleted. A
    /// service that keeps no snapshots copies nothing: a named `baseline` is then missing from the
    /// target, so the fork refuses to publish a target that could not start.
    pub(crate) async fn copy(
        mut self,
        target: &OwnedAgentId,
        stage_id: Uuid,
        baseline: Option<&FilesystemSnapshotName>,
    ) -> Result<Copied, CallError> {
        let stage = AgentSnapshots::agent(target, AgentFingerprint(stage_id));
        let Some(ticket) = self.ticket.as_mut() else {
            return Ok(Copied {
                ticket: None,
                stage_id,
                baseline: match baseline {
                    Some(name) => Baseline::Missing(name.clone()),
                    None => Baseline::NotChecked,
                },
            });
        };
        ticket.stage = Some(stage.clone());
        let (core, from) = (Arc::clone(&ticket.core), ticket.from.clone());
        core.calls.copy(&from, &stage).await?;
        let baseline = match baseline {
            Some(name) => match missing(&core, &stage, name).await {
                Ok(false) => Baseline::Present,
                Ok(true) => Baseline::Missing(name.clone()),
                Err(error) => {
                    return Err(match error {
                        ReadError::Failed(failed) => CallError::Failed(failed),
                        ReadError::Stopped => {
                            CallError::Stopped(crate::filesystem_snapshot::Withdrawal::Stopped)
                        }
                        ReadError::Corrupt(error) => {
                            CallError::Failed(crate::filesystem_snapshot::Failed::new(error))
                        }
                    });
                }
            },
            None => Baseline::NotChecked,
        };
        Ok(Copied {
            ticket: self.ticket.take(),
            stage_id,
            baseline,
        })
    }
}

/// The snapshots of a fork attempt, copied into its stage.
pub(crate) struct Copied {
    ticket: Option<ForkTicket>,
    stage_id: Uuid,
    baseline: Baseline,
}

impl Copied {
    /// Whether the stage holds the snapshot of the baseline of the target.
    pub(crate) fn baseline(&self) -> Baseline {
        self.baseline.clone()
    }

    /// Runs `publish` with the permission to publish the stage, which the publication consumes.
    /// The attempt is in its publication before the first poll of `publish`. The service then
    /// decides about the snapshots of the stage: a live target with the instance id of the stage
    /// is published; a live target with another id makes the stage lost, so its snapshots are
    /// deleted; a refused publication asks `live_instance` for the instance id of the live
    /// target, and only another id makes the stage lost; an unknown outcome leaves the snapshots
    /// of the stage, counted.
    pub(crate) async fn publish<'a, T, E>(
        mut self,
        publish: impl FnOnce(StagePublication) -> BoxFuture<'a, PublishFound<T, E>>,
        live_instance: impl FnOnce() -> BoxFuture<'a, Option<AgentFingerprint>>,
    ) -> Result<T, E> {
        if let Some(ticket) = &self.ticket {
            ticket
                .core
                .registry
                .apply(|state| transitions::fork_publishing(state, &ticket.flight));
        }
        let publication = StagePublication::new(self.stage_id, PublicationKey(()));
        let (found, answer) = match publish(publication).await {
            PublishFound::Published(value) => (ForkFound::Published, Ok(value)),
            PublishFound::Live(live, value) => (ForkFound::Live(live), Ok(value)),
            PublishFound::Refused(value) => {
                (ForkFound::RefusedThenLive(live_instance().await), Ok(value))
            }
            PublishFound::Unknown(error) => (ForkFound::Unknown, Err(error)),
        };
        self.found(found);
        answer
    }

    /// A reconciliation before the publication found a live target with `instance_id`. The
    /// service compares it with the stage id, as [`Copied::publish`] does.
    pub(crate) fn lost_to(mut self, instance_id: AgentFingerprint) {
        self.found(ForkFound::Live(instance_id));
    }

    /// Records what the attempt found, which its end applies.
    fn found(&mut self, found: ForkFound) {
        if let Some(ticket) = self.ticket.as_mut() {
            ticket.outcome = Some(decisions::fork_outcome(
                found,
                AgentFingerprint(self.stage_id),
            ));
        }
    }
}

/// Holds the snapshots of `from` for the fork attempt `flight`, waiting while a delete of the
/// snapshots of `from` is pending or runs, and while another attempt of the same flight runs.
/// A shutdown ends the wait with [`ForkStopped`].
pub(super) async fn begin(
    core: &Arc<Core>,
    from: &AgentSnapshots,
    flight: Flight,
) -> Result<ForkCopy, ForkStopped> {
    let attempts = futures::stream::unfold(Some(false), |state| {
        let flight = flight.clone();
        async move {
            let waited = state?;
            let mut changed = core.registry.subscribe();
            if core.shutdown.is_cancelled() {
                return Some((Some(Err(ForkStopped)), None));
            }
            if core
                .registry
                .apply(|state| transitions::fork_began(state, from, &flight))
            {
                return Some((Some(Ok(waited)), None));
            }
            tokio::select! {
                changed = changed.changed() => match changed {
                    Ok(()) => Some((None, Some(true))),
                    Err(_) => Some((Some(Err(ForkStopped)), None)),
                },
                () = core.shutdown.cancelled() => Some((Some(Err(ForkStopped)), None)),
            }
        }
    });
    let waited = futures::StreamExt::next(&mut std::pin::pin!(futures::StreamExt::filter_map(
        attempts,
        |began| async move { began }
    )))
    .await
    .unwrap_or(Err(ForkStopped))?;
    Ok(ForkCopy {
        ticket: Some(ForkTicket {
            core: Arc::clone(core),
            from: from.clone(),
            flight,
            stage: None,
            outcome: None,
        }),
        waited,
    })
}

/// A fork attempt of a service that keeps no snapshots: it holds nothing and copies nothing.
pub(super) fn without_snapshots() -> ForkCopy {
    ForkCopy {
        ticket: None,
        waited: false,
    }
}

/// Whether the store does not hold the snapshot `name` of `agent`: one `stat`, bounded by the
/// limit of a check of the store.
pub(super) async fn missing(
    core: &Core,
    agent: &AgentSnapshots,
    name: &FilesystemSnapshotName,
) -> Result<bool, ReadError> {
    let Ok(name) = store_name(name) else {
        return Ok(true);
    };
    core.calls
        .stat(agent, &name, core.settings.store_check_limit())
        .await
        .map(|info| info.is_none())
}

/// The flight of a fork request: its target and the hash of the request.
pub(crate) fn flight(target: &AgentId, request_hash: [u8; 32]) -> Flight {
    (target.clone(), request_hash)
}
