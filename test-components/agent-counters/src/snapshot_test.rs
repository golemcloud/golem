use golem_rust::{agent_definition, agent_implementation};
use serde::{Deserialize, Serialize};

use crate::busy_loop;

#[agent_definition(snapshotting = "every(10)")]
trait SnapshotCounter {
    fn new(id: String) -> Self;
    fn increment(&mut self) -> u32;
    fn busy_for(&mut self, millis: u32) -> u32;
    fn oplog_heavy(&mut self, entries: u32) -> u32;
    fn get(&self) -> u32;
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
            let mut buf = [0u8; 4];
            wstd::rand::get_random_bytes(&mut buf);
            self.count = self.count.wrapping_add(u32::from_le_bytes(buf));
        }
        self.count
    }

    fn get(&self) -> u32 {
        self.count
    }

    fn was_recovered_from_snapshot(&self) -> bool {
        self.recovered_from_snapshot
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        Ok(self.count.to_le_bytes().to_vec())
    }

    async fn load_snapshot(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        if bytes.len() == 4 {
            self.count = u32::from_le_bytes(bytes.try_into().unwrap());
            self.recovered_from_snapshot = true;
            Ok(())
        } else {
            Err(format!("Invalid snapshot size: {}", bytes.len()))
        }
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
