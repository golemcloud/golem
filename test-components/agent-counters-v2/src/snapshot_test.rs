use crate::busy_loop;
use golem_rust::{agent_definition, agent_implementation};
use serde::{Deserialize, Serialize};

#[agent_definition(snapshotting = "enabled")]
trait SnapshotCounter {
    fn new(id: String) -> Self;
    fn increment(&mut self) -> u32;
    fn busy_for(&mut self, millis: u32) -> u32;
    fn oplog_heavy(&mut self, entries: u32) -> u32;
    fn get(&self) -> u32;
    /// Differs between agent-counters and agent-counters-v2, so a replay across
    /// a build change is detectable.
    fn component_version(&self) -> u32;
    fn was_recovered_from_snapshot(&self) -> bool;
}

struct SnapshotCounterImpl {
    count: u32,
    _id: String,
    recovered_from_snapshot: bool,
}

#[agent_implementation]
impl SnapshotCounter for SnapshotCounterImpl {
    fn new(id: String) -> Self {
        Self {
            _id: id,
            count: 0,
            recovered_from_snapshot: false,
        }
    }

    fn increment(&mut self) -> u32 {
        self.count += 1;
        self.count
    }

    fn busy_for(&mut self, millis: u32) -> u32 {
        let _ = busy_loop(millis);
        self.count += 1;
        self.count
    }

    fn oplog_heavy(&mut self, entries: u32) -> u32 {
        for _ in 0..entries {
            let random = golem_rust::wasip3::random::random::get_random_u64();
            self.count = self.count.wrapping_add(random as u32);
        }
        self.count
    }

    fn get(&self) -> u32 {
        self.count
    }

    fn component_version(&self) -> u32 {
        2
    }

    fn was_recovered_from_snapshot(&self) -> bool {
        self.recovered_from_snapshot
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        Ok(self.count.to_le_bytes().to_vec())
    }

    async fn load_snapshot(
        bytes: Vec<u8>,
        context: golem_rust::agentic::SnapshotRestoreContext,
    ) -> Result<Self, String> {
        let count = if bytes.len() == 4 {
            u32::from_le_bytes(bytes.try_into().unwrap())
        } else {
            return Err(format!("Invalid snapshot size: {}", bytes.len()));
        };
        let mut parameters = golem_rust::agentic::DirectAgentInput::new(context.parameters)
            .map_err(|e| e.to_string())?;
        let id = parameters.take::<String>().map_err(|e| e.to_string())?;
        parameters.finish().map_err(|e| e.to_string())?;
        Ok(Self {
            count,
            _id: id,
            recovered_from_snapshot: true,
        })
    }
}

#[agent_definition(snapshotting = "enabled")]
trait JsonSnapshotCounter {
    fn new(id: String) -> Self;
    fn increment(&mut self) -> u32;
    fn get(&self) -> u32;
}

#[derive(Serialize, Deserialize)]
struct JsonSnapshotCounterImpl {
    count: u32,
    #[serde(skip)]
    _id: String,
}

#[agent_implementation]
impl JsonSnapshotCounter for JsonSnapshotCounterImpl {
    fn new(id: String) -> Self {
        Self { _id: id, count: 0 }
    }

    fn increment(&mut self) -> u32 {
        self.count += 1;
        self.count
    }

    fn get(&self) -> u32 {
        self.count
    }
}
