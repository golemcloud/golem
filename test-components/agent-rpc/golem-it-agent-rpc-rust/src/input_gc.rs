//! Snapshot restoration varies disposal only, without changing completed stream reads.
use crate::StreamingRpcTargetClient;
use golem_rust::agentic::AgentStream;
use golem_rust::{PromiseId, agent_definition, agent_implementation};
use serde::{Deserialize, Serialize};

#[agent_definition(snapshotting = "enabled")]
pub trait InputGc {
    fn new(name: String) -> Self;
    /// Drop before the next host call (0), after it (1), or retain the stream (2).
    fn configure(&mut self, live: u32, replay: u32);
    fn promise(&self) -> PromiseId;
    async fn run_to_end(&mut self);
    fn wait(&self, gate: PromiseId);
    async fn run(
        &mut self,
        reads: u32,
        unwrap: bool,
        omit_read_after_restore: bool,
        gate: PromiseId,
    );
    fn inspect(&self) -> (bool, u32, u32);
    async fn continuation(&mut self) -> Vec<u32>;
}

#[derive(Serialize, Deserialize)]
struct Snapshot {
    name: String,
    replay: u32,
}

struct InputGcImpl {
    state: Snapshot,
    schedule: u32,
    restored: bool,
    reads: u32,
    retained: Vec<AgentStream<u32>>,
}

#[agent_implementation]
impl InputGc for InputGcImpl {
    fn new(name: String) -> Self {
        Self {
            state: Snapshot { name, replay: 2 },
            schedule: 2,
            restored: false,
            reads: 0,
            retained: Vec::new(),
        }
    }

    fn configure(&mut self, live: u32, replay: u32) {
        self.schedule = live;
        self.state.replay = replay;
    }

    fn promise(&self) -> PromiseId {
        golem_rust::create_promise()
    }

    async fn run_to_end(&mut self) {
        let stream = StreamingRpcTargetClient::get(self.state.name.clone())
            .produce(vec![20, 21, 22])
            .await;
        if self.schedule == 2 {
            self.retained.push(stream);
        } else {
            drop(stream);
        }
    }

    fn wait(&self, gate: PromiseId) {
        golem_rust::blocking_await_promise(&gate);
    }

    async fn run(
        &mut self,
        reads: u32,
        unwrap: bool,
        omit_read_after_restore: bool,
        gate: PromiseId,
    ) {
        let target = StreamingRpcTargetClient::get(self.state.name.clone());
        let mut stream = target.produce(vec![20, 21, 22]).await;
        if unwrap {
            stream = AgentStream::from_raw(stream.into_raw().await.unwrap());
        }
        if !(self.restored && omit_read_after_restore) {
            for index in 0..reads {
                assert_eq!(stream.next().await.unwrap(), Some(20 + index));
                self.reads += 1;
            }
        }
        let mut stream = Some(stream);
        if self.schedule == 0 {
            drop(stream.take());
        }
        golem_rust::retry::remove_retry_policy("input-gc-unused");
        match (self.schedule, stream) {
            (2, Some(stream)) => self.retained.push(stream),
            (_, stream) => drop(stream),
        }
        golem_rust::blocking_await_promise(&gate);
    }

    fn inspect(&self) -> (bool, u32, u32) {
        (self.restored, self.reads, self.retained.len() as u32)
    }

    async fn continuation(&mut self) -> Vec<u32> {
        StreamingRpcTargetClient::get(self.state.name.clone())
            .produce(vec![30, 31, 32])
            .await
            .collect()
            .await
            .unwrap()
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(&self.state).map_err(|err| err.to_string())
    }

    async fn load_snapshot(
        bytes: Vec<u8>,
        _context: golem_rust::agentic::SnapshotRestoreContext,
    ) -> Result<Self, String> {
        let state: Snapshot = serde_json::from_slice(&bytes).map_err(|err| err.to_string())?;
        Ok(Self {
            schedule: state.replay,
            state,
            restored: true,
            reads: 0,
            retained: Vec::new(),
        })
    }
}
