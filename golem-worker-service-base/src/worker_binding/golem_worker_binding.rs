use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};

use golem_common::model::ComponentId;
use rib::Expr;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
#[serde(rename_all = "camelCase")]
pub struct GolemWorkerBinding {
    pub component_id: ComponentId,
    pub worker_name: Expr,
    pub idempotency_key: Option<Expr>,
    pub response: ResponseMapping,
}

// ResponseMapping will consist of actual logic such as invoking worker functions
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct ResponseMapping(pub Expr);
