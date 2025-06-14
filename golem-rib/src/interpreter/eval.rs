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

use crate::{
    DefaultWorkerNameGenerator, Expr, GenerateWorkerName, RibCompilationError, RibCompiler,
    RibCompilerConfig, RibComponentFunctionInvoke, RibInput, RibResult, RibRuntimeError,
};
use std::sync::Arc;

pub struct RibEvalConfig {
    compiler_config: RibCompilerConfig,
    rib_input: RibInput,
    function_invoke: Arc<dyn RibComponentFunctionInvoke + Sync + Send>,
    generate_worker_name: Arc<dyn GenerateWorkerName + Sync + Send>,
}

impl RibEvalConfig {
    pub fn new(
        compiler_config: RibCompilerConfig,
        rib_input: RibInput,
        function_invoke: Arc<dyn RibComponentFunctionInvoke + Sync + Send>,
        generate_worker_name: Option<Arc<dyn GenerateWorkerName + Sync + Send>>,
    ) -> Self {
        RibEvalConfig {
            compiler_config,
            rib_input,
            function_invoke,
            generate_worker_name: generate_worker_name
                .unwrap_or_else(|| Arc::new(DefaultWorkerNameGenerator)),
        }
    }
}

pub struct RibEvaluator {
    pub config: RibEvalConfig,
}

impl RibEvaluator {
    pub fn new(config: RibEvalConfig) -> Self {
        RibEvaluator { config }
    }

    pub async fn eval(self, rib: &str) -> Result<RibResult, RibEvaluationError> {
        let expr = Expr::from_text(rib).map_err(RibEvaluationError::ParseError)?;
        let config = self.config.compiler_config;
        let compiler = RibCompiler::new(config);
        let compiled = compiler.compile(expr.clone())?;

        let result = crate::interpret(
            compiled.byte_code,
            self.config.rib_input,
            self.config.function_invoke,
            Some(self.config.generate_worker_name.clone()),
        )
        .await?;

        Ok(result)
    }
}

#[derive(Debug)]
pub enum RibEvaluationError {
    ParseError(String),
    CompileError(RibCompilationError),
    RuntimeError(RibRuntimeError),
}

impl From<RibCompilationError> for RibEvaluationError {
    fn from(error: RibCompilationError) -> Self {
        RibEvaluationError::CompileError(error)
    }
}

impl From<RibRuntimeError> for RibEvaluationError {
    fn from(error: RibRuntimeError) -> Self {
        RibEvaluationError::RuntimeError(error)
    }
}
