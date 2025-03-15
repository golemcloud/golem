use serde::{Deserialize, Serialize};
use std::time::SystemTime;

use golem_wasm_ast::analysis::{analysed_type, AnalysedType};
use golem_wasm_rpc::{IntoValue, Value};
use rand::distr::{Alphanumeric, SampleString};
use rand::rng;

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

impl IntoValue for Data {
    fn into_value(self) -> Value {
        Value::Record(vec![
            Value::String(self.id),
            Value::String(self.name),
            Value::String(self.desc),
            Value::U64(self.timestamp),
        ])
    }

    fn get_type() -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("id", analysed_type::str()),
            analysed_type::field("name", analysed_type::str()),
            analysed_type::field("desc", analysed_type::str()),
            analysed_type::field("timestamp", analysed_type::u64()),
        ])
    }
}
