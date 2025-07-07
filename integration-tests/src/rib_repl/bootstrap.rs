use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::base_model::{ComponentId, TargetWorkerId};
use golem_rib_repl::WorkerFunctionInvoke;
use golem_rib_repl::{ReplComponentDependencies, RibDependencyManager};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_ast::analysis::{AnalysedExport, AnalysedType};
use golem_wasm_rpc::{Value, ValueAndType};
use rib::{ComponentDependency, ComponentDependencyKey, ParsedFunctionName, ParsedFunctionReference, ParsedFunctionSite};
use std::path::Path;
use uuid::Uuid;
use golem_wasm_ast::analysis::analysed_type::str;

pub struct TestRibReplDependencyManager {
    dependencies: EnvBasedTestDependencies,
}

impl TestRibReplDependencyManager {
    pub fn new(dependencies: EnvBasedTestDependencies) -> Self {
        Self { dependencies }
    }
}

#[async_trait]
impl RibDependencyManager for TestRibReplDependencyManager {
    async fn get_dependencies(&self) -> anyhow::Result<ReplComponentDependencies> {
        Err(anyhow!("test will need to run with a single component"))
    }

    async fn add_component(
        &self,
        _source_path: &Path,
        component_name: String,
    ) -> anyhow::Result<ComponentDependency> {
        let component_id = self
            .dependencies
            .admin()
            .component(component_name.as_str())
            .store()
            .await;

        let metadata = self
            .dependencies
            .admin()
            .get_latest_component_metadata(&component_id)
            .await;

        let component_dependency_key = ComponentDependencyKey {
            component_name,
            component_id: component_id.0,
            root_package_name: metadata.root_package_name,
            root_package_version: metadata.root_package_version,
        };

        Ok(ComponentDependency::new(
            component_dependency_key,
            metadata.exports,
        ))
    }
}

pub struct TestRibReplStaticDependencyManager {
    dependencies: EnvBasedTestDependencies,
    static_exports: Vec<AnalysedExport>,
}


impl TestRibReplStaticDependencyManager {
    pub fn new(dependencies: EnvBasedTestDependencies, static_exports: Vec<AnalysedExport>) -> Self {
        Self { dependencies, static_exports }
    }
}

#[async_trait]
impl RibDependencyManager for TestRibReplStaticDependencyManager {
    async fn get_dependencies(&self) -> anyhow::Result<ReplComponentDependencies> {
        Err(anyhow!("test will need to run with a single component"))
    }

    async fn add_component(
        &self,
        _source_path: &Path,
        component_name: String,
    ) -> anyhow::Result<ComponentDependency> {
        let component_id = self
            .dependencies
            .admin()
            .component(component_name.as_str())
            .store()
            .await;

        let metadata = self
            .dependencies
            .admin()
            .get_latest_component_metadata(&component_id)
            .await;

        let component_dependency_key = ComponentDependencyKey {
            component_name,
            component_id: component_id.0,
            root_package_name: metadata.root_package_name,
            root_package_version: metadata.root_package_version,
        };

        Ok(ComponentDependency::new(
            component_dependency_key,
            self.static_exports.clone()
        ))
    }
}

// Embedded RibFunctionInvoke implementation
pub struct TestRibReplWorkerFunctionInvoke {
    embedded_worker_executor: EnvBasedTestDependencies,
}

impl TestRibReplWorkerFunctionInvoke {
    pub fn new(embedded_worker_executor: EnvBasedTestDependencies) -> Self {
        Self {
            embedded_worker_executor,
        }
    }
}


#[async_trait]
impl WorkerFunctionInvoke for TestRibReplWorkerFunctionInvoke {
    async fn invoke(
        &self,
        component_id: Uuid,
        _component_name: &str,
        worker_name: Option<String>,
        function_name: &str,
        args: Vec<ValueAndType>,
        _return_type: Option<AnalysedType>,
    ) -> anyhow::Result<Option<ValueAndType>> {
        let target_worker_id = worker_name
            .map(|w| TargetWorkerId {
                component_id: ComponentId(component_id),
                worker_name: Some(w),
            })
            .unwrap_or_else(|| TargetWorkerId {
                component_id: ComponentId(component_id),
                worker_name: None,
            });

        self.embedded_worker_executor
            .admin()
            .invoke_and_await_typed(target_worker_id, function_name, args)
            .await
            .map_err(|e| anyhow!("Failed to invoke function: {:?}", e))
    }
}


// TODO; this won't be required if we have a composed component that does the redirection
pub struct TestRibReplAgenticWorkerFunctionInvoke {
    embedded_worker_executor: EnvBasedTestDependencies,
}

impl TestRibReplAgenticWorkerFunctionInvoke {
    pub fn new(embedded_worker_executor: EnvBasedTestDependencies) -> Self {
        Self {
            embedded_worker_executor,
        }
    }
}

#[async_trait]
impl WorkerFunctionInvoke for TestRibReplAgenticWorkerFunctionInvoke {
    async fn invoke(
        &self,
        component_id: Uuid,
        _component_name: &str,
        worker_name: Option<String>,
        function_name: &str,
        args: Vec<ValueAndType>,
        _return_type: Option<AnalysedType>,
    ) -> anyhow::Result<Option<ValueAndType>> {

        let target_worker_id = worker_name
            .map(|w| TargetWorkerId {
                component_id: ComponentId(component_id),
                worker_name: Some(w),
            })
            .unwrap_or_else(|| TargetWorkerId {
                component_id: ComponentId(component_id),
                worker_name: None,
            });

        let parsed_function_name = ParsedFunctionName::parse(function_name)
            .map_err(|e| anyhow!("Failed to parse function name: {:?}", e))?;

        match parsed_function_name.site {
            ParsedFunctionSite::PackagedInterface { package,interface, .. } if package == "agentic" && interface == "simulated-agents" => {
                match parsed_function_name.function {
                    ParsedFunctionReference::RawResourceConstructor { resource } => {
                        let new_function_name = "golem:agentic-guest/guest.{agent.new}".to_string();
                        let worker_name = target_worker_id.worker_name.clone().unwrap();

                        let agent_name = ValueAndType::new(golem_wasm_rpc::Value::String(resource), str());
                        let agent_id =  ValueAndType::new(golem_wasm_rpc::Value::String(worker_name), str());

                        self.embedded_worker_executor
                            .admin()
                            .invoke_and_await_typed(target_worker_id, new_function_name.as_str(), vec![agent_name, agent_id])
                            .await
                            .map_err(|e| anyhow!("Failed to invoke function: {:?}", e))

                    }
                    reference => {
                       match reference {
                           ParsedFunctionReference::RawResourceMethod { method , ..} => {
                               let new_function_name = "golem:agentic-guest/guest.{[method]agent.invoke}".to_string();

                               let mut new_args = vec![];

                               new_args.push(args[0].clone());

                               let agent_name =
                                   ValueAndType::new(Value::String(method), str());

                               new_args.push(agent_name);

                               let new_list =
                                   args[1..].iter().map(|x| x.value.clone()).collect();

                               let args_list = ValueAndType::new(Value::List(new_list), str());

                               new_args.push(args_list);

                               self.embedded_worker_executor
                                   .admin()
                                   .invoke_and_await_typed(target_worker_id, new_function_name.as_str(), new_args)
                                   .await
                                   .map_err(|e| anyhow!("Failed to invoke function: {:?}", e))

                           }
                           _ => {
                               Err(anyhow!("Unsupported function reference: {:?}", reference))
                           }
                       }
                    }
                }
            }

            _ =>  {

                self.embedded_worker_executor
                    .admin()
                    .invoke_and_await_typed(target_worker_id, function_name, args)
                    .await
                    .map_err(|e| anyhow!("Failed to invoke function: {:?}", e))
            }
        }

    }
}