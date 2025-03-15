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

pub use byte_code::*;
pub use compiler_output::*;
use golem_wasm_ast::analysis::AnalysedExport;
pub use ir::*;
use std::fmt::Display;
pub use type_with_unit::*;
pub use worker_functions_in_rib::*;

use crate::rib_compilation_error::RibCompilationError;
use crate::type_registry::FunctionTypeRegistry;
use crate::{Expr, GlobalVariableTypeSpec, InferredExpr, RibInputTypeInfo, RibOutputTypeInfo};

mod byte_code;
mod compiler_output;
mod desugar;
mod ir;
mod type_with_unit;
mod worker_functions_in_rib;

pub fn compile(
    expr: Expr,
    export_metadata: &Vec<AnalysedExport>,
) -> Result<CompilerOutput, RibError> {
    compile_with_restricted_global_variables(expr, export_metadata, None, &vec![])
}

// Rib allows global input variables, however, we can choose to fail compilation
// if they don't fall under a pre-defined set of global variables. If nothing is specified,
// then it implies, any names can be a global variable in Rib. Example: `foo`.
// Along with this, we can explicitly specify the types of certain global variables using `GlobalVariableTypeSpec`.
// `GlobalVariableTypeSpec` is a compiler configuration that customises it's behaviour.
// Example:  request.path.*` should be always a `string`.
// Not all global variables require a type specification.
pub fn compile_with_restricted_global_variables(
    expr: Expr,
    export_metadata: &Vec<AnalysedExport>,
    allowed_global_variables: Option<Vec<String>>,
    global_variable_type_spec: &Vec<GlobalVariableTypeSpec>,
) -> Result<CompilerOutput, RibError> {
    for info in global_variable_type_spec {
        if !info.variable_id.is_global() {
            return Err(RibError::InternalError(format!(
                "variable {} in the type spec is not a global variable",
                info.variable_id
            )));
        }
    }

    let type_registry = FunctionTypeRegistry::from_export_metadata(export_metadata);
    let inferred_expr = InferredExpr::from_expr(expr, &type_registry, global_variable_type_spec)?;

    let function_calls_identified =
        WorkerFunctionsInRib::from_inferred_expr(&inferred_expr, &type_registry)?;

    let global_input_type_info = RibInputTypeInfo::from_expr(&inferred_expr)?;

    let output_type_info = RibOutputTypeInfo::from_expr(&inferred_expr)?;

    if let Some(allowed_global_variables) = &allowed_global_variables {
        let mut un_allowed_variables = vec![];

        for (name, _) in global_input_type_info.types.iter() {
            if !allowed_global_variables.contains(name) {
                un_allowed_variables.push(name.clone());
            }
        }

        if !un_allowed_variables.is_empty() {
            return Err(RibError::InternalError(format!(
                "Global variables not allowed: {}. Allowed: {}",
                un_allowed_variables.join(", "),
                allowed_global_variables.join(", ")
            )));
        }
    }

    let byte_code = RibByteCode::from_expr(&inferred_expr).map_err(|e| {
        RibError::InternalError(format!(
            "failed to convert inferred expression to byte code: {}",
            e
        ))
    })?;

    Ok(CompilerOutput {
        worker_invoke_calls: function_calls_identified,
        byte_code,
        rib_input_type_info: global_input_type_info,
        rib_output_type_info: Some(output_type_info),
    })
}

#[derive(Debug, Clone)]
pub enum RibError {
    InternalError(String),
    RibCompilationError(RibCompilationError),
}

impl From<RibCompilationError> for RibError {
    fn from(err: RibCompilationError) -> Self {
        RibError::RibCompilationError(err)
    }
}

impl Display for RibError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RibError::InternalError(msg) => write!(f, "rib internal error: {}", msg),
            RibError::RibCompilationError(err) => write!(f, "{}", err),
        }
    }
}
