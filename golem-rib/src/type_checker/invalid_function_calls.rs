// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::call_type::CallType;
use crate::type_checker::missing_fields::find_missing_fields_in_record;
use crate::{type_checker, ComponentDependencies, FunctionName};
use crate::{Expr, ExprVisitor, FunctionCallError};
use golem_wasm_ast::analysis::AnalysedType;

// While we have a dedicated generic phases (refer submodules) within type_checker module,
// we have this special phase to grab errors in the context function calls.
// This is grab as many errors as possible.
// Refer `FunctionCallTypeCheckError`.
#[allow(clippy::result_large_err)]
pub fn check_invalid_function_calls(expr: &mut Expr) -> Result<(), FunctionCallError> {
    let mut visitor = ExprVisitor::bottom_up(expr);

    while let Some(expr) = visitor.pop_front() {
        if let Expr::Call {
            call_type, args, ..
        } = &expr
        {
            match call_type {
                CallType::Function {
                    component_info,
                    function_name,
                    ..
                } => {
                    if component_info.is_none() {
                        return Err(FunctionCallError::InvalidFunctionCall {
                            function_name: function_name.function.name_pretty().to_string(),
                            expr: expr.clone(),
                            message: "function call without component. make sure component functions are called on an instance".to_string(),
                        });
                    }
                }
                _ => {}
            }
        }
    }

    Ok(())
}
