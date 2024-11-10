use serde::{Deserialize, Serialize};

include!(concat!(env!("OUT_DIR"), "/src/lib.rs"));

#[cfg(test)]
test_r::enable!();

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultComponentOwner;
