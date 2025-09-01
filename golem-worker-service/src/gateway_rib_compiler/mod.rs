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

use golem_common::model::agent::AgentType;
use golem_wasm_ast::analysis::AnalysedExport;
use rib::{
    CompilerOutput, ComponentDependency, ComponentDependencyKey, Expr, GlobalVariableTypeSpec,
    InferredType, InterfaceName, Path, RibCompilationError, RibCompiler, RibCompilerConfig,
};

// A wrapper over ComponentDependency which is coming from rib-module
// to attach agent types to it.
pub struct ComponentDependencyWithAgentInfo {
    agent_types: Vec<AgentType>,
    component_dependency: ComponentDependency,
}

impl ComponentDependencyWithAgentInfo {
    pub fn new(
        component_dependency_key: ComponentDependencyKey,
        component_exports: Vec<AnalysedExport>,
        agent_types: Vec<AgentType>,
    ) -> Self {
        Self {
            agent_types,
            component_dependency: ComponentDependency::new(
                component_dependency_key,
                component_exports,
            ),
        }
    }
}

// A wrapper service over original Rib Compiler concerning
// the details of the worker bridge.
pub trait WorkerServiceRibCompiler {
    fn compile(
        rib: &Expr,
        component_dependency: &[ComponentDependencyWithAgentInfo],
    ) -> Result<CompilerOutput, RibCompilationError>;
}

pub struct DefaultWorkerServiceRibCompiler;

impl WorkerServiceRibCompiler for DefaultWorkerServiceRibCompiler {
    fn compile(
        rib: &Expr,
        component_dependency: &[ComponentDependencyWithAgentInfo],
    ) -> Result<CompilerOutput, RibCompilationError> {
        let agent_types = component_dependency
            .iter()
            .enumerate()
            .map(|cd| (cd.0, cd.1.agent_types.clone()))
            .collect::<Vec<_>>();

        let mut custom_instance_spec = vec![];

        for (_component_index, agent_types) in agent_types {
            for agent_type in agent_types {
                custom_instance_spec.push(rib::CustomInstanceSpec {
                    instance_name: agent_type.type_name.clone(),
                    parameter_types: vec![], // TODO; needed for compiler check, otherwise runtime error
                    interface_name: Some(InterfaceName {
                        name: agent_type.type_name,
                        version: None,
                    }),
                });
            }
        }

        let component_dependency = component_dependency
            .iter()
            .map(|cd| cd.component_dependency.clone())
            .collect::<Vec<_>>();

        let rib_input_spec = vec![
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
        ];

        let compiler_config =
            RibCompilerConfig::new(component_dependency, rib_input_spec, custom_instance_spec);

        let compiler = RibCompiler::new(compiler_config);

        compiler.compile(rib.clone())
    }
}
