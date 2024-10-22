use std::collections::VecDeque;
use crate::{DynamicParsedFunctionName, Expr, FunctionTypeRegistry};
use crate::call_type::CallType;

#[derive(Debug, Clone)]
pub struct InferredExpr(pub Expr);

impl InferredExpr {
    pub fn from_expr(
        expr: &Expr,
        function_type_registry: &FunctionTypeRegistry,
    ) -> Result<InferredExpr, String> {
        let mut mutable_expr = expr.clone();
        mutable_expr
            .infer_types(function_type_registry)
            .map_err(|err| err.join("\n"))?;
        Ok(InferredExpr(mutable_expr))
    }

    pub fn worker_function_calls(&self) -> Vec<DynamicParsedFunctionName> {
        let expr = self.0.clone();
        let mut queue = VecDeque::new();
        queue.push_back(self.0.clone());
        if let expr = 
    }
}
