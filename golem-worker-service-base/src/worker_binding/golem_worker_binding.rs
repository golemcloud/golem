use std::collections::HashMap;
use bincode::{Decode, Encode};
use golem_wasm_ast::core::Expr;
use serde::{Deserialize, Serialize};
use golem_common::model::TemplateId;
use crate::expr::Expr;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
#[serde(rename_all = "camelCase")]
pub struct GolemWorkerBinding {
    pub template: TemplateId,
    pub worker_id: Expr,
    pub function_name: String,
    pub function_params: Vec<Expr>,
    pub response: Option<ResponseMapping>,
}


// TODO; https://github.com/golemcloud/golem/issues/318
// This will make GolemWorkerBidning generic for all protocols
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct ResponseMapping {
    pub body: Expr,   // ${function.return}
    pub status: Expr, // "200" or if ${response.body.id == 1} "200" else "400"
    pub headers: HashMap<String, Expr>,
}

pub trait HasGolemWorkerBindings {
    fn get_golem_worker_bindings(&self) -> Vec<GolemWorkerBinding>;
}

