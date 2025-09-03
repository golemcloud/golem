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

use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::NonSuccessfulExit;
use crate::log::logln;
use crate::model::component::ComponentView;
use crate::model::text::component::ComponentReplStartedView;
use crate::model::text::fmt::log_error;
use crate::model::{
    ComponentName, ComponentNameMatchKind, ComponentVersionSelection, IdempotencyKey, WorkerName,
};
use anyhow::bail;
use async_trait::async_trait;
use golem_common::model::agent::{DataSchema, ElementSchema, TextReference};
use golem_rib_repl::{
    ReplComponentDependencies, RibDependencyManager, RibRepl, RibReplConfig, WorkerFunctionInvoke,
};
use golem_wasm_ast::analysis::analysed_type::{field, list, option, record, str, u8, variant};
use golem_wasm_ast::analysis::{AnalysedType, NameOptionTypePair};
use golem_wasm_rpc::json::OptionallyValueAndTypeJson;
use golem_wasm_rpc::ValueAndType;
use rib::{ComponentDependency, ComponentDependencyKey};
use std::path::Path;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
pub struct RibReplHandler {
    ctx: Arc<Context>,
}

impl RibReplHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn cmd_repl(
        &self,
        component_name: Option<ComponentName>,
        component_version: Option<u64>,
    ) -> anyhow::Result<()> {
        let selected_components = self
            .ctx
            .component_handler()
            .must_select_components_by_app_dir_or_name(component_name.as_ref())
            .await?;

        let component_name = {
            if selected_components.component_names.len() == 1 {
                selected_components.component_names[0].clone()
            } else {
                self.ctx
                    .interactive_handler()
                    .select_component_for_repl(selected_components.component_names.clone())?
            }
        };

        // NOTE: we pre-create the ReplDependencies, because trying to do it in RibDependencyManager::get_dependencies
        //       results in thread safety errors on the path when cargo component could be called for client building
        let component = self
            .ctx
            .component_handler()
            .component_by_name_with_auto_deploy(
                selected_components.project.as_ref(),
                ComponentNameMatchKind::App,
                &component_name,
                component_version.map(|v| v.into()),
            )
            .await?;

        let component_dependency_key = ComponentDependencyKey {
            component_name: component.component_name.0.clone(),
            component_id: component.versioned_component_id.component_id,
            root_package_name: component.metadata.root_package_name().clone(),
            root_package_version: component.metadata.root_package_version().clone(),
        };

        // The REPL has to know about the instance parameters to auto-populate the constructor arguments
        // It has to also take into account how AgentId parser considers these input.
        let custom_instance_spec = component
            .metadata
            .agent_types()
            .iter()
            .map(|agent_type| {
                let constructor_args = {
                    match &agent_type.constructor.input_schema {
                        DataSchema::Tuple(element_schemas) => element_schemas
                            .elements
                            .iter()
                            .map(|x| match &x.schema {
                                ElementSchema::ComponentModel(component_model_elem_schema) => {
                                    component_model_elem_schema.element_type.clone()
                                }
                                ElementSchema::UnstructuredText(_) => {
                                    // the constructor args for unstructured text is just a text
                                    // Ex: foo or [en]foo, where en is the language code
                                    str()
                                }
                                ElementSchema::UnstructuredBinary(_) => {
                                    // Example argument in constructor: [image/png]"iVBORw0KGA"
                                    // The REPL can re-inspect the schema again and generate a proper base64
                                    str()
                                }
                            })
                            .collect::<Vec<AnalysedType>>(),
                        DataSchema::Multimodal(named_element_schemas) => {
                            // the value is wrapped in names
                            named_element_schemas
                                .elements
                                .iter()
                                .map(|x| {
                                    let name = &x.name;

                                    let analysed_type = match &x.schema {
                                        ElementSchema::ComponentModel(
                                            component_model_elem_schema,
                                        ) => component_model_elem_schema.element_type.clone(),
                                        ElementSchema::UnstructuredText(_) => str(),
                                        ElementSchema::UnstructuredBinary(_) => str(),
                                    };

                                    let name_and_type = NameOptionTypePair {
                                        name: name.clone(),
                                        typ: Some(analysed_type),
                                    };

                                    variant(vec![name_and_type])
                                })
                                .collect::<Vec<_>>()
                        }
                    }
                };
                rib::CustomInstanceSpec::new(
                    agent_type.type_name.to_string(),
                    constructor_args,
                    Some(rib::InterfaceName {
                        name: agent_type.type_name.to_string(),
                        version: None,
                    }),
                )
            })
            .collect::<Vec<_>>();

        self.ctx
            .set_rib_repl_dependencies(ReplComponentDependencies {
                component_dependencies: vec![ComponentDependency::new(
                    component_dependency_key,
                    component.metadata.exports().to_vec(),
                )],
                custom_instance_spec,
            })
            .await;

        let mut repl = RibRepl::bootstrap(RibReplConfig {
            history_file: Some(self.ctx.rib_repl_history_file().await?),
            dependency_manager: Arc::new(self.clone()),
            worker_function_invoke: Arc::new(self.clone()),
            printer: None,
            component_source: None,
            prompt: None,
            command_registry: None,
        })
        .await?;

        logln("");

        self.ctx
            .log_handler()
            .log_view(&ComponentReplStartedView(ComponentView::new(
                self.ctx.show_sensitive(),
                component,
            )));

        logln("");

        repl.run().await;
        Ok(())
    }
}

#[async_trait]
impl RibDependencyManager for RibReplHandler {
    async fn get_dependencies(&self) -> anyhow::Result<ReplComponentDependencies> {
        Ok(self.ctx.get_rib_repl_dependencies().await)
    }

    async fn add_component(
        &self,
        _source_path: &Path,
        _component_name: String,
    ) -> anyhow::Result<ComponentDependency> {
        unreachable!("add_component should not be used in CLI")
    }
}

#[async_trait]
impl WorkerFunctionInvoke for RibReplHandler {
    async fn invoke(
        &self,
        component_id: Uuid,
        component_name: &str,
        worker_name: &str,
        function_name: &str,
        args: Vec<ValueAndType>,
        _return_type: Option<AnalysedType>,
    ) -> anyhow::Result<Option<ValueAndType>> {
        let worker_name = WorkerName::from(worker_name);

        let component = self
            .ctx
            .component_handler()
            .component(
                None,
                component_id.into(),
                Some(ComponentVersionSelection::ByWorkerName(&WorkerName(
                    worker_name.to_string(),
                ))),
            )
            .await?;

        let Some(component) = component else {
            log_error(format!("Component {component_name} not found"));
            bail!(NonSuccessfulExit);
        };

        let arguments: Vec<OptionallyValueAndTypeJson> = args
            .into_iter()
            .map(|vat| vat.try_into().unwrap())
            .collect();

        let result = self
            .ctx
            .worker_handler()
            .invoke_worker(
                &component,
                &worker_name,
                function_name,
                arguments,
                IdempotencyKey::new(),
                false,
                None,
            )
            .await?
            .unwrap();

        Ok(result.result)
    }
}
