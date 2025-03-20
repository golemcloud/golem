use serde::{Deserialize, Serialize};

pub const OPENAPI_YAML: &[u8; 210840] = include_bytes!("../openapi/golem-service.yaml");
include!(concat!(env!("OUT_DIR"), "/src/lib.rs"));

#[cfg(test)]
test_r::enable!();

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultComponentOwner;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultPluginOwner;
