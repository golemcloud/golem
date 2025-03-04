// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::call_type::CallType;
use crate::rib_compilation_error::RibCompilationError;
use crate::{
    DynamicParsedFunctionName, Expr, FunctionTypeRegistry, GlobalVariableTypeSpec, RegistryKey,
};
use std::collections::{HashSet, VecDeque};

#[derive(Debug, Clone)]
pub struct InferredExpr(Expr);

impl InferredExpr {
    pub fn get_expr(&self) -> &Expr {
        &self.0
    }

    pub fn from_expr(
        expr: Expr,
        function_type_registry: &FunctionTypeRegistry,
        type_spec: &Vec<GlobalVariableTypeSpec>,
    ) -> Result<InferredExpr, RibCompilationError> {
        let mut mutable_expr = expr;

        mutable_expr.infer_types(function_type_registry, type_spec)?;

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
                Expr::Call {
                    call_type:
                        CallType::Function {
                            function_name,
                            worker,
                        },
                    ..
                } => {
                    worker_calls.push(function_name.clone());

                    if let Some(worker) = worker {
                        queue.push_back(worker);
                    }
                }
                _ => expr.visit_children_bottom_up(&mut queue),
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
