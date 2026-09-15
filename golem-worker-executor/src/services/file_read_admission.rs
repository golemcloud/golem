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

use crate::services::golem_config::FileReadConfig;
use golem_common::model::OwnedAgentId;
use golem_common::model::filesystem::FileReadError;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::{Instant, timeout_at};

/// Executor-local admission, shared by all requests, including requests for absent agents.
#[derive(Debug)]
pub struct FileReadAdmission {
    state: Mutex<State>,
    /// Queued requests beyond the single active turn.
    max_queued_per_agent: usize,
    max_outstanding: usize,
    duration: Duration,
}

#[derive(Debug, Default)]
struct State {
    agents: HashMap<OwnedAgentId, AgentReads>,
    outstanding: usize,
}

#[derive(Debug)]
struct AgentReads {
    outstanding: usize,
    active: Arc<Semaphore>,
}

/// A bounded reservation made before any activation or initialization work is polled.
#[derive(Debug)]
pub struct FileReadReservation {
    admission: Arc<FileReadAdmission>,
    agent: OwnedAgentId,
    active: Arc<Semaphore>,
    deadline: Instant,
}

/// Keeps both the per-agent turn and global reservation until the read's terminal observation.
pub(crate) struct FileReadPermit {
    _active: OwnedSemaphorePermit,
    reservation: FileReadReservation,
}

impl Default for FileReadAdmission {
    fn default() -> Self {
        Self::from(&FileReadConfig::default())
    }
}

impl From<&FileReadConfig> for FileReadAdmission {
    fn from(config: &FileReadConfig) -> Self {
        Self::new(
            config.max_queued_per_agent,
            config.max_outstanding,
            config.timeout,
        )
    }
}

impl FileReadAdmission {
    pub(crate) fn deadline(&self, arrival: Instant) -> Result<Instant, FileReadError> {
        arrival
            .checked_add(self.duration)
            .ok_or(FileReadError::DeadlineExceeded)
    }

    pub(crate) fn new(
        max_queued_per_agent: usize,
        max_outstanding: usize,
        duration: Duration,
    ) -> Self {
        Self {
            state: Mutex::new(State::default()),
            max_queued_per_agent,
            max_outstanding,
            duration,
        }
    }

    /// Rejects immediately when full; never creates a task to wait for capacity.
    ///
    /// `arrival` is captured at the executor boundary, before validation and worker lookup. Keep
    /// the returned reservation through all asynchronous work and reuse its deadline, never a
    /// fresh duration after acquiring the agent's turn.
    pub fn reserve(
        self: &Arc<Self>,
        agent: OwnedAgentId,
        arrival: Instant,
    ) -> Result<FileReadReservation, FileReadError> {
        let deadline = self.deadline(arrival)?;
        let mut state = self.state.lock().unwrap();
        if Instant::now() >= deadline {
            return Err(FileReadError::DeadlineExceeded);
        }
        if state.outstanding >= self.max_outstanding {
            return Err(FileReadError::ResourceExhausted);
        }
        let reads = state
            .agents
            .entry(agent.clone())
            .or_insert_with(|| AgentReads {
                outstanding: 0,
                active: Arc::new(Semaphore::new(1)),
            });
        if reads.outstanding > self.max_queued_per_agent {
            return Err(FileReadError::ResourceExhausted);
        }
        reads.outstanding += 1;
        let active = reads.active.clone();
        state.outstanding += 1;
        Ok(FileReadReservation {
            admission: self.clone(),
            agent,
            active,
            deadline,
        })
    }
}

impl FileReadReservation {
    pub(crate) fn deadline(&self) -> Instant {
        self.deadline
    }

    /// Waits for the agent's single turn without extending the arrival deadline.
    /// Dropping this future releases its queued reservation synchronously.
    pub(crate) async fn acquire(self) -> Result<FileReadPermit, FileReadError> {
        let active = timeout_at(self.deadline, self.active.clone().acquire_owned())
            .await
            .map_err(|_| FileReadError::DeadlineExceeded)?
            .expect("file read turn semaphore is never closed");
        // A ready semaphore can win timeout_at's first poll even after the deadline.
        if Instant::now() >= self.deadline {
            return Err(FileReadError::DeadlineExceeded);
        }
        Ok(FileReadPermit {
            _active: active,
            reservation: self,
        })
    }
}

impl FileReadPermit {
    pub(crate) fn deadline(&self) -> Instant {
        self.reservation.deadline()
    }
}

impl Drop for FileReadReservation {
    fn drop(&mut self) {
        let mut state = self.admission.state.lock().unwrap();
        let reads = state.agents.get_mut(&self.agent).expect("registered read");
        reads.outstanding -= 1;
        if reads.outstanding == 0 {
            state.agents.remove(&self.agent);
        }
        state.outstanding -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::poll;
    use golem_common::model::AgentId;
    use golem_common::model::component::ComponentId;
    use golem_common::model::environment::EnvironmentId;
    use test_r::{test, timeout};

    fn agent() -> OwnedAgentId {
        OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId::from_agent_name_string(ComponentId::new(), "read").unwrap(),
        )
    }

    fn assert_empty(admission: &FileReadAdmission) {
        let state = admission.state.lock().unwrap();
        assert_eq!(state.outstanding, 0);
        assert!(state.agents.is_empty());
    }

    #[test]
    fn configured_limits_override_defaults() {
        let admission = Arc::new(FileReadAdmission::from(&FileReadConfig {
            max_queued_per_agent: 0,
            max_outstanding: 2,
            timeout: Duration::from_secs(250),
        }));
        let first_agent = agent();
        let arrival = Instant::now();
        let first = admission.reserve(first_agent.clone(), arrival).unwrap();
        assert_eq!(first.deadline(), arrival + Duration::from_secs(250));
        assert!(matches!(
            admission.reserve(first_agent, arrival),
            Err(FileReadError::ResourceExhausted)
        ));
        let second = admission.reserve(agent(), arrival).unwrap();
        assert!(matches!(
            admission.reserve(agent(), arrival),
            Err(FileReadError::ResourceExhausted)
        ));
        drop((first, second));
        assert_empty(&admission);
    }

    #[test]
    #[timeout("30s")]
    async fn arrival_deadline_and_queue_capacity_include_unstarted_work() {
        let corpus: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../golem-service-base/tests/fixtures/http-handlers/corpus.json"
        )))
        .unwrap();
        let id = "lifecycle-read-queue-bounded";
        let case = corpus["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["id"] == id)
            .expect("lifecycle-read-queue-bounded");
        let queued_count: usize = case["input"]["events"][1]
            .as_str()
            .unwrap()
            .strip_prefix("queued-reads:")
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(queued_count, 16, "{id}");
        let admission = Arc::new(FileReadAdmission::default());
        let agent = agent();
        let arrival = Instant::now() - Duration::from_secs(10);
        let reservation = admission.reserve(agent.clone(), arrival).unwrap();
        assert_eq!(reservation.deadline(), arrival + Duration::from_secs(60));
        let mut queued = Vec::new();
        for _ in 0..queued_count {
            queued.push(admission.reserve(agent.clone(), arrival).unwrap());
        }
        assert!(
            matches!(
                admission.reserve(agent.clone(), arrival),
                Err(FileReadError::ResourceExhausted)
            ),
            "{id}"
        );
        let active = reservation.acquire().await.unwrap();
        assert_eq!(active.deadline(), arrival + Duration::from_secs(60));
        let mut waiting = Box::pin(queued.pop().unwrap().acquire());
        assert!(poll!(&mut waiting).is_pending());
        drop(waiting);
        // A cancelled queued future frees a slot even while the first request is active.
        drop(admission.reserve(agent, arrival).unwrap());
        drop(queued);
        drop(active);
        assert_empty(&admission);
    }

    #[test]
    #[timeout("30s")]
    async fn global_limit_bounds_distinct_agents_and_idle_registry_entries() {
        let admission = Arc::new(FileReadAdmission::default());
        let arrival = Instant::now();
        let mut reservations = Vec::new();
        for _ in 0..128 {
            reservations.push(admission.reserve(agent(), arrival).unwrap());
        }
        assert!(matches!(
            admission.reserve(agent(), arrival),
            Err(FileReadError::ResourceExhausted)
        ));
        assert_eq!(admission.state.lock().unwrap().agents.len(), 128);
        drop(reservations.pop());
        drop(admission.reserve(agent(), arrival).unwrap());
        drop(reservations);
        assert_empty(&admission);
        for _ in 0..256 {
            drop(admission.reserve(agent(), arrival).unwrap());
        }
        assert_empty(&admission);
    }

    #[test]
    #[timeout("30s")]
    async fn cancellation_at_grant_keeps_single_agent_serialized() {
        let admission = Arc::new(FileReadAdmission::default());
        let agent = agent();
        let arrival = Instant::now();
        let active = admission
            .reserve(agent.clone(), arrival)
            .unwrap()
            .acquire()
            .await
            .unwrap();
        let mut second = Box::pin(admission.reserve(agent.clone(), arrival).unwrap().acquire());
        let mut third = Box::pin(admission.reserve(agent.clone(), arrival).unwrap().acquire());
        assert!(poll!(&mut second).is_pending());
        assert!(poll!(&mut third).is_pending());
        drop(active); // The semaphore grants to second, but its future has not observed the grant.
        drop(second);
        let third = third.await.unwrap();
        let mut fourth = Box::pin(admission.reserve(agent, arrival).unwrap().acquire());
        assert!(poll!(&mut fourth).is_pending());
        drop(third);
        drop(fourth.await.unwrap());
        assert_empty(&admission);
    }

    #[test]
    #[timeout("30s")]
    async fn expired_queue_releases_capacity_without_extending_deadline() {
        let admission = Arc::new(FileReadAdmission::new(1, 2, Duration::from_millis(20)));
        let agent = agent();
        let arrival = Instant::now();
        let active = admission
            .reserve(agent.clone(), arrival)
            .unwrap()
            .acquire()
            .await
            .unwrap();
        let queued = admission.reserve(agent.clone(), arrival).unwrap();
        assert!(matches!(
            queued.acquire().await,
            Err(FileReadError::DeadlineExceeded)
        ));
        assert_eq!(admission.state.lock().unwrap().outstanding, 1);
        assert!(matches!(
            admission.reserve(agent.clone(), arrival),
            Err(FileReadError::DeadlineExceeded)
        ));
        drop(active);
        drop(
            admission
                .reserve(agent, Instant::now())
                .unwrap()
                .acquire()
                .await
                .unwrap(),
        );
        assert_empty(&admission);
    }

    #[test]
    fn reserve_rechecks_deadline_after_waiting_for_state_lock() {
        let admission = Arc::new(FileReadAdmission::new(1, 2, Duration::from_millis(20)));
        let state = admission.state.lock().unwrap();
        let arrival = Instant::now();
        let blocked_admission = admission.clone();
        let reserve = std::thread::spawn(move || blocked_admission.reserve(agent(), arrival));

        std::thread::sleep(Duration::from_millis(50));
        drop(state);

        assert!(matches!(
            reserve.join().unwrap(),
            Err(FileReadError::DeadlineExceeded)
        ));
        assert_empty(&admission);
    }
}
