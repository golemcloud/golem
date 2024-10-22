use std::collections::{HashSet, VecDeque};
use crate::{DynamicParsedFunctionName, Expr, FunctionTypeRegistry, RegistryKey};
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

    // Only a fully inferred Rib can reliably tell us what are the exact
    // function calls.
    pub fn worker_invoke_calls(&self) -> Vec<DynamicParsedFunctionName> {
        let mut worker_calls = vec![];
        let mut queue = VecDeque::new();
        queue.push_back(&self.0);
        while let Some(expr) = queue.pop_back() {
            match expr {
                Expr::Call(CallType::Function(function_name), _, _) => {
                    worker_calls.push(function_name.clone())
                }
                _ => expr.visit_children_bottom_up(&mut queue)
            }
        }

        worker_calls
    }

    pub fn worker_invoke_registry_keys(&self) -> HashSet<RegistryKey> {
        let worker_calls = self.worker_invoke_calls();

        let mut registry_keys = HashSet::new();

        for call in worker_calls {
            let keys = RegistryKey::registry_keys_of_function(&call);
            registry_keys.extend(keys)
        }

        registry_keys

    }
}
