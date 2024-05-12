use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};

use golem_common::model::ComponentId;

use crate::expression::Expr;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
#[serde(rename_all = "camelCase")]
pub struct GolemWorkerBinding {
    pub component: Expr,
    pub worker_id: Expr,
    pub function_name: Expr,
    pub function_params: Vec<Expr>,
    pub response: Option<ResponseMapping>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct ResponseMapping(pub Expr);
