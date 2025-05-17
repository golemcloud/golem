use serde::{Deserialize, Serialize};
use std::time::SystemTime;

use golem_wasm_rpc_derive::IntoValue;
use rand::distr::{Alphanumeric, SampleString};
use rand::rng;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, IntoValue)]
pub struct Data {
    pub id: String,
    pub name: String,
    pub desc: String,
    pub timestamp: u64,
}

impl Data {
    pub fn generate() -> Self {
        fn generate_random_string(len: usize) -> String {
            let mut rng = rng();
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
