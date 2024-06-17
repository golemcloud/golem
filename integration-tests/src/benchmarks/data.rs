use golem_wasm_rpc::Value;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

use rand::distributions::{Alphanumeric, DistString};
use rand::thread_rng;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Data {
    pub id: String,
    pub name: String,
    pub desc: String,
    pub timestamp: u64,
}

impl Data {
    pub fn generate() -> Self {
        fn generate_random_string(len: usize) -> String {
            let mut rng = thread_rng();
            Alphanumeric.sample_string(&mut rng, len)
        }

        Self {
            id: generate_random_string(256),
            name: generate_random_string(512),
            desc: generate_random_string(1024),
            timestamp: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        }
    }

    pub fn generate_list(count: usize) -> Vec<Self> {
        (0..count).map(|_| Self::generate()).collect()
    }
}

impl From<Data> for Value {
    fn from(data: Data) -> Self {
        Value::Record(vec![
            Value::String(data.id),
            Value::String(data.name),
            Value::String(data.desc),
            Value::U64(data.timestamp),
        ])
    }
}
