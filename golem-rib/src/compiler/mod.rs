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
use std::error::Error;
use std::fmt::Display;
pub use type_with_unit::*;
pub use worker_functions_in_rib::*;

use crate::rib_type_error::RibTypeError;
use crate::type_registry::FunctionTypeRegistry;
use crate::{
    Expr, GlobalVariableTypeSpec, InferredExpr, RibInputTypeInfo, RibOutputTypeInfo, VariableId,
};

mod byte_code;
mod compiler_output;
mod desugar;
mod ir;
mod type_with_unit;
mod worker_functions_in_rib;

// TODO: Change this to proper comments
// Rib allows global input variables, however, we can choose to fail compilation
// if they don't fall under a pre-defined set of global variables. If nothing is specified,
// then it implies, any names can be a global variable in Rib. Example: `foo`.
// Along with this, we can explicitly specify the types of certain global variables using `GlobalVariableTypeSpec`.
// `GlobalVariableTypeSpec` is a compiler configuration that customises it's behaviour.
// Example:  request.path.*` should be always a `string`.
// Not all global variables require a type specification.
#[derive(Default)]
pub struct CompilerConfig {
    component_metadata: Vec<AnalysedExport>,
    rib_global_input: Vec<GlobalVariableTypeSpec>,
}

impl CompilerConfig {
    pub fn new(
        component_metadata: Vec<AnalysedExport>,
        global_variable_type_spec: Vec<GlobalVariableTypeSpec>,
    ) -> CompilerConfig {
        CompilerConfig {
            component_metadata,
            rib_global_input: global_variable_type_spec,
        }
    }
}

#[derive(Default)]
pub struct Compiler {
    config: CompilerConfig,
}

impl Compiler {
    pub fn new(config: CompilerConfig) -> Compiler {
        Compiler { config }
    }

    pub fn with_component_metadata(&mut self, component_metadata: Vec<AnalysedExport>) {
        self.config.component_metadata = component_metadata
    }

    pub fn with_global_variables(&mut self, global_variables: Vec<GlobalVariableTypeSpec>) {
        self.config.rib_global_input = global_variables
    }

    pub fn compile(&self, expr: Expr) -> Result<CompilerOutput, RibCompilationError> {
        let type_registry =
            FunctionTypeRegistry::from_export_metadata(&self.config.component_metadata);
        let inferred_expr =
            InferredExpr::from_expr(expr, &type_registry, &self.config.rib_global_input)?;

        let function_calls_identified =
            WorkerFunctionsInRib::from_inferred_expr(&inferred_expr, &type_registry)?;

        // The types that are tagged as global input in the script
        let global_input_type_info = RibInputTypeInfo::from_expr(&inferred_expr)?;
        let output_type_info = RibOutputTypeInfo::from_expr(&inferred_expr)?;

        // allowed_global_variables
        let allowed_global_variables: Vec<String> = self
            .config
            .rib_global_input
            .iter()
            .map(|x| x.variable())
            .collect::<Vec<_>>();

        let mut unidentified_global_inputs = vec![];

        for (name, _) in global_input_type_info.types.iter() {
            if !allowed_global_variables.contains(name) {
                unidentified_global_inputs.push(name.clone());
            }
        }

        if !unidentified_global_inputs.is_empty() {
            return Err(RibCompilationError::UnsupportedGlobalInput {
                invalid_global_inputs: unidentified_global_inputs,
                valid_global_inputs: allowed_global_variables,
            });
        }

        let byte_code = RibByteCode::from_expr(&inferred_expr)?;

        Ok(CompilerOutput {
            worker_invoke_calls: function_calls_identified,
            byte_code,
            rib_input_type_info: global_input_type_info,
            rib_output_type_info: Some(output_type_info),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RibCompilationError {
    // Bytecode generation errors should ideally never occur.
    // They are considered programming errors that indicate some part of type checking
    // or inference needs to be fixed.
    ByteCodeGenerationFail(RibByteCodeGenerationError),

    // RibTypeError is a type error that occurs during type inference.
    // This is a typical compilation error, such as: expected u32, found str.
    RibTypeError(RibTypeError),

    // This captures only the syntax parse errors in a Rib script.
    InvalidSyntax(String),

    // This occurs when the Rib script includes global inputs that cannot be
    // fulfilled. For example, if Rib is used from a REPL, the only valid global input will be `env`.
    // If it is used from the Golem API gateway, it is  `request`.
    // If the user specifies a global input such as `foo`
    // (e.g., the compiler will treat `foo` as a global input in a Rib script like `my-worker-function(foo)`),
    // it will fail compilation with this error.
    // Note: the type inference phase will still be happy with this Rib script;
    // we perform this validation as an extra step at the end to allow clients of `golem-rib`
    // to decide what global inputs are valid.
    UnsupportedGlobalInput {
        invalid_global_inputs: Vec<String>,
        valid_global_inputs: Vec<String>,
    },

    // A typical use of static analysis in Rib is to identify all the valid worker functions.
    // If this analysis phase fails, it typically indicates a bug in the Rib compiler.
    RibStaticAnalysisError(String),
}

impl From<RibByteCodeGenerationError> for RibCompilationError {
    fn from(err: RibByteCodeGenerationError) -> Self {
        RibCompilationError::RibStaticAnalysisError(err.to_string())
    }
}

impl From<RibTypeError> for RibCompilationError {
    fn from(err: RibTypeError) -> Self {
        RibCompilationError::RibTypeError(err)
    }
}

impl Display for RibCompilationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RibCompilationError::RibStaticAnalysisError(msg) => {
                write!(f, "rib static analysis error: {}", msg)
            }
            RibCompilationError::RibTypeError(err) => write!(f, "{}", err),
            RibCompilationError::InvalidSyntax(msg) => write!(f, "invalid rib syntax: {}", msg),
            RibCompilationError::UnsupportedGlobalInput {
                invalid_global_inputs,
                valid_global_inputs,
            } => {
                write!(
                    f,
                    "unsupported global input variables: {}. expected: {}",
                    invalid_global_inputs.join(", "),
                    valid_global_inputs.join(", ")
                )
            }
            RibCompilationError::ByteCodeGenerationFail(e) => {
                write!(f, "rib byte code generation error: {}", e)
            }
        }
    }
}

impl Error for RibCompilationError {}
