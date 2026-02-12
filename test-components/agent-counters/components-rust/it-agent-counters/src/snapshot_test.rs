use golem_rust::{agent_definition, agent_implementation};

#[agent_definition(snapshotting = "enabled")]
trait SnapshotCounter {
    fn new(id: String) -> Self;
    fn increment(&mut self) -> u32;
    fn get(&self) -> u32;
}

struct SnapshotCounterImpl {
    count: u32,
    _id: String,
}

#[agent_implementation]
impl SnapshotCounter for SnapshotCounterImpl {
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

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        Ok(self.count.to_le_bytes().to_vec())
    }

    async fn load_snapshot(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        if bytes.len() == 4 {
            self.count = u32::from_le_bytes(bytes.try_into().unwrap());
            Ok(())
        } else {
            Err(format!("Invalid snapshot size: {}", bytes.len()))
        }
    }
}
