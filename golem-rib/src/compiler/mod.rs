// Copyright 2024 Golem Cloud
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

pub use byte_code::*;
use golem_wasm_ast::analysis::AnalysedExport;
pub use ir::*;
pub use type_with_unit::*;
pub use compiler_output::*;
pub use worker_invoke_calls::*;

use crate::type_registry::FunctionTypeRegistry;
use crate::{Expr, InferredExpr, RibInputTypeInfo};

mod byte_code;
mod desugar;
mod worker_invoke_calls;
mod ir;
mod type_with_unit;
mod compiler_output;

pub fn compile(
    expr: &Expr,
    export_metadata: &Vec<AnalysedExport>,
) -> Result<CompilerOutput, String> {
    compile_with_limited_globals(expr, export_metadata, None)
}

// Rib allows global input variables, however, we can choose to fail compilation
// if they don't fall under a pre-defined set of global variables.
// There is no restriction imposed to the type of this variable.
pub fn compile_with_limited_globals(
    expr: &Expr,
    export_metadata: &Vec<AnalysedExport>,
    allowed_global_variables: Option<Vec<String>>,
) -> Result<CompilerOutput, String> {
    let type_registry = FunctionTypeRegistry::from_export_metadata(export_metadata);
    let inferred_expr = InferredExpr::from_expr(expr, &type_registry)?;
    let function_calls_identified =
        WorkerInvokeCallsInRib::from_inferred_expr(&inferred_expr, &type_registry)?;

    let global_input_type_info =
        RibInputTypeInfo::from_expr(&inferred_expr.0).map_err(|e| format!("Error: {}", e))?;

    if let Some(allowed_global_variables) = &allowed_global_variables {
        let mut un_allowed_variables = vec![];

        for (name, _) in global_input_type_info.types.iter() {
            if !allowed_global_variables.contains(name) {
                un_allowed_variables.push(name.clone());
            }
        }

        if !un_allowed_variables.is_empty() {
            return Err(format!(
                "Global variables not allowed: {}. Allowed: {}",
                un_allowed_variables.join(", "),
                allowed_global_variables.join(", ")
            ));
        }
    }

    let byte_code = RibByteCode::from_expr(inferred_expr.0.clone())?;

    Ok(CompilerOutput {
        worker_invoke_calls: function_calls_identified,
        byte_code,
        global_input_type_info,
    })
}
