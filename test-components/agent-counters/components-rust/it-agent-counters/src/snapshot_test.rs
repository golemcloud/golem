use golem_rust::{agent_definition, agent_implementation};

#[agent_definition(snapshotting = "enabled")]
trait SnapshotCounter {
    fn new(id: String) -> Self;
    fn increment(&mut self) -> u32;
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
