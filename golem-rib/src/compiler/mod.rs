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

use crate::type_registry::FunctionTypeRegistry;
use crate::{Expr, InferredExpr, RibInputTypeInfo};
use golem_api_grpc::proto::golem::rib::CompilerOutput as ProtoCompilerOutput;
use crate::compiler::function_calls::WorkerInvokeCallsInRib;

mod byte_code;
mod desugar;
mod ir;
mod type_with_unit;
mod function_calls;

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
        function_calls: function_calls_identified,
        byte_code,
        global_input_type_info,
    })
}

#[derive(Debug, Clone)]
pub struct CompilerOutput {
    pub function_calls: WorkerInvokeCallsInRib,
    pub byte_code: RibByteCode,
    pub global_input_type_info: RibInputTypeInfo,
}

impl TryFrom<ProtoCompilerOutput> for CompilerOutput {
    type Error = String;

    fn try_from(value: ProtoCompilerOutput) -> Result<Self, Self::Error> {
        let proto_rib_input = value.rib_input.ok_or("Missing rib_input")?;
        let proto_byte_code = value.byte_code.ok_or("Missing byte_code")?;
        let rib_input = RibInputTypeInfo::try_from(proto_rib_input)?;
        let byte_code = RibByteCode::try_from(proto_byte_code)?;

        Ok(CompilerOutput {
            byte_code,
            global_input_type_info: rib_input,
        })
    }
}

impl From<CompilerOutput> for ProtoCompilerOutput {
    fn from(value: CompilerOutput) -> Self {
        ProtoCompilerOutput {
            byte_code: Some(golem_api_grpc::proto::golem::rib::RibByteCode::from(
                value.byte_code,
            )),
            rib_input: Some(golem_api_grpc::proto::golem::rib::RibInputType::from(
                value.global_input_type_info,
            )),
        }
    }
}
