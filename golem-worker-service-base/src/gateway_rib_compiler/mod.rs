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

use golem_wasm_ast::analysis::AnalysedExport;
use rib::{CompilerOutput, Expr, GlobalVariableTypeSpec, InferredType, Path, RibCompilationError};

// A wrapper service over original Rib Compiler concerning
// the details of the worker bridge.
pub trait WorkerServiceRibCompiler {
    fn compile(
        rib: &Expr,
        export_metadata: &[AnalysedExport],
    ) -> Result<CompilerOutput, RibCompilationError>;
}

pub struct DefaultWorkerServiceRibCompiler;

impl WorkerServiceRibCompiler for DefaultWorkerServiceRibCompiler {
    fn compile(
        rib: &Expr,
        export_metadata: &[AnalysedExport],
    ) -> Result<CompilerOutput, RibCompilationError> {
        rib::compile_with_restricted_global_variables(
            rib.clone(),
            &export_metadata.to_vec(),
            Some(vec!["request".to_string()]),
            &vec![
                GlobalVariableTypeSpec::new(
                    "request",
                    Path::from_elems(vec!["path"]),
                    InferredType::string(),
                ),
                GlobalVariableTypeSpec::new(
                    "request",
                    Path::from_elems(vec!["query"]),
                    InferredType::string(),
                ),
                // `request.headers.*` or `request.header.*` should be a `string`.
                GlobalVariableTypeSpec::new(
                    "request",
                    Path::from_elems(vec!["headers"]),
                    InferredType::string(),
                ),
                GlobalVariableTypeSpec::new(
                    "request",
                    Path::from_elems(vec!["header"]),
                    InferredType::string(),
                ),
            ],
        )
    }
}
