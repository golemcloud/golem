use golem_common::uri::oss::urn::WorkerUrn;
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct TryUpdateAllWorkersResult {
    pub triggered: Vec<WorkerUrn>,
    pub failed: Vec<WorkerUrn>,
}
