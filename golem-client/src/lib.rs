use golem_common::model::plugin::ComponentPluginScope;
use golem_common::model::{Empty, ProjectId};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fmt::{Display, Formatter};

include!(concat!(env!("OUT_DIR"), "/src/lib.rs"));

#[cfg(test)]
test_r::enable!();

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultComponentOwner;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultPluginOwner;
