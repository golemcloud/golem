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

use golem_common::model::component_metadata::ComponentMetadata;
use golem_wasm_rpc::IntoValue;
use rib::{
    CompilerOutput, ComponentDependency, ComponentDependencyKey, Expr, GlobalVariableTypeSpec,
    InferredType, InterfaceName, Path, RibCompilationError, RibCompiler, RibCompilerConfig,
};
use uuid::Uuid;

// A wrapper over ComponentDependency which is coming from rib-module
// to attach agent types to it.
pub struct ComponentDependencyWithAgentInfo {
    component_metadata: ComponentMetadata,
    component_dependency: ComponentDependency,
}

impl ComponentDependencyWithAgentInfo {
    pub fn new(
        component_dependency_key: ComponentDependencyKey,
        component_metadata: ComponentMetadata,
    ) -> Self {
        let exports = component_metadata.exports().to_vec();
        Self {
            component_metadata,
            component_dependency: ComponentDependency::new(component_dependency_key, exports),
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
        let mut custom_instance_spec = vec![];

        for dep in component_dependency {
            let metadata = &dep.component_metadata;

            for agent_type in metadata.agent_types() {
                let wrapper_function = metadata
                    .find_wrapper_function_by_agent_constructor(&agent_type.type_name)
                    .map_err(RibCompilationError::RibStaticAnalysisError)?
                    .ok_or_else(|| {
                        RibCompilationError::RibStaticAnalysisError(format!(
                            "Missing static WIT wrapper for constructor of agent type {}",
                            agent_type.type_name
                        ))
                    })?;

                custom_instance_spec.push(rib::CustomInstanceSpec {
                    instance_name: agent_type.wrapper_type_name(),
                    parameter_types: wrapper_function
                        .analysed_export
                        .parameters
                        .iter()
                        .map(|p| p.typ.clone())
                        .collect(),
                    interface_name: Some(InterfaceName {
                        name: agent_type.wrapper_type_name(),
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
            // TODO:
            // What we actually want is request.request_id to be uuid, but this says "all children of request.request_id" are uuid.
            // At runtime we expect request.request_id.value to be accessed.
            GlobalVariableTypeSpec::new(
                "request",
                Path::from_elems(vec!["request_id"]),
                (&Uuid::get_type()).into(),
            ),
        ];

        let compiler_config =
            RibCompilerConfig::new(component_dependency, rib_input_spec, custom_instance_spec);

        let compiler = RibCompiler::new(compiler_config);

        compiler.compile(rib.clone())
    }
}
