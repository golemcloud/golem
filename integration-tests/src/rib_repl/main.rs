use golem_rib_repl::{ComponentSource, RibRepl, RibReplConfig};
use golem_test_framework::config::{
    EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, TestDependencies,
};
use integration_tests::rib_repl::bootstrap::*;
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use golem_client::model::AnalysedType;
use golem_wasm_ast::analysis::analysed_type::handle;
use golem_wasm_ast::analysis::{AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult, AnalysedInstance, AnalysedResourceId, AnalysedResourceMode};
use golem_wasm_rpc::{Value, ValueAndType, WitType, WitTypeNode, WitValue};
use golem_wasm_rpc::json::TypeAnnotatedValueJsonExtensions;
use golem_wasm_rpc::protobuf::{TypeAnnotatedValue, Val};
use rib::RibResult;

#[tokio::main]
async fn main() {
    let deps = EnvBasedTestDependencies::new(EnvBasedTestDependenciesConfig::new()).await;

    let component_name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "shopping-cart".to_string());

    let wasm_path = deps.component_directory().join(format!("{component_name}.wasm"));

    let repl_config = RibReplConfig {
        history_file: None,
        dependency_manager: Arc::new(TestRibReplDependencyManager::new(deps.clone())),
        worker_function_invoke: Arc::new(TestRibReplWorkerFunctionInvoke::new(deps.clone())),
        printer: None,
        component_source: Some(ComponentSource {
            component_name: component_name.clone(),
            source_path: wasm_path.clone(),
        }),
        prompt: None,
        command_registry: None,
    };

    let mut rib_repl = RibRepl::bootstrap(repl_config)
        .await
        .expect("Failed to bootstrap REPL");

    rib_repl.run().await;

    // if let Err(_) = rib_repl.execute("let x = instance()").await {
    //     eprintln!("Failed to execute instance()");
    //     rib_repl.run().await;
    //     return;
    // }
    //
    // match rib_repl.execute("x.discover-agent-definitions()").await {
    //     Ok(Some(result)) => {
    //         let analysed_functions = extract_agent_definitions(result);
    //
    //         let exports = AnalysedInstance {
    //             name: "golem:agentic/simulated-agents".to_string(),
    //             functions: analysed_functions,
    //         };
    //
    //         let repl_config_with_exports = RibReplConfig {
    //             history_file: None,
    //             dependency_manager: Arc::new(TestRibReplStaticDependencyManager::new(
    //                 deps.clone(),
    //                 vec![AnalysedExport::Instance(exports)],
    //             )),
    //             worker_function_invoke: Arc::new(TestRibReplAgenticWorkerFunctionInvoke::new(deps.clone())),
    //             printer: None,
    //             component_source: Some(ComponentSource {
    //                 component_name,
    //                 source_path: wasm_path,
    //             }),
    //             prompt: None,
    //             command_registry: None,
    //         };
    //
    //         let mut rib_repl = RibRepl::bootstrap(repl_config_with_exports)
    //             .await
    //             .expect("Failed to bootstrap REPL with exports");
    //
    //         rib_repl.run().await;
    //     }
    //
    //     _ => {
    //         eprintln!("Falling back to default REPL (agent discovery failed)");
    //         rib_repl.run().await;
    //     }
    // }
}


fn extract_agent_definitions(result: RibResult) -> Vec<AnalysedFunction> {
    match result {
        RibResult::Val(value_and_type) => match value_and_type.value {
            Value::List(values) => match value_and_type.typ {
                AnalysedType::List(inner_type) => match inner_type.inner.as_ref() {
                    AnalysedType::Record(record_type) => {
                        let methods_index = record_type.fields.iter().position(|f| f.name == "methods")
                            .expect("Expected 'methods' field");
                        let name_index = record_type.fields.iter().position(|f| f.name == "agent-name")
                            .expect("Expected 'agent-name' field");
                        let methods_type = &record_type.fields[methods_index].typ;

                        let mut functions = Vec::new();

                        for agent in values {
                            match agent {
                                Value::Record(fields) => {
                                    let agent_name_val = fields.get(name_index)
                                        .expect("Missing 'agent-name' value");
                                    let methods_val = fields.get(methods_index)
                                        .expect("Missing 'methods' value");

                                    let resource_fn = get_agent_resource_analysed_function(agent_name_val);
                                    let method_fns = get_agent_methods(methods_val, methods_type)
                                        .iter()
                                        .map(|info| get_agent_resource_method_analysed_function(get_str(agent_name_val), info))
                                        .collect::<Vec<_>>();

                                    functions.push(resource_fn);
                                    functions.extend(method_fns);
                                }
                                _ => panic!("Expected agent record, got: {:?}", agent),
                            }
                        }

                        functions
                    }
                    _ => panic!("Expected record type inside list, got: {:?}", inner_type.inner),
                },
                _ => panic!("Expected list type, got: {:?}", value_and_type.typ),
            },
            _ => panic!("Expected list value, got: {:?}", value_and_type.value),
        },
        _ => panic!("Expected value result, got: {:?}", result),
    }
}

fn get_str(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        _ => panic!("Expected a string value, got: {:?}", value),
    }
}

fn get_agent_resource_analysed_function(name: &Value) -> AnalysedFunction {

   let name = get_str(name);

    let resource_name = format!("[constructor]{}", name);

    AnalysedFunction {
        name:resource_name,
        parameters: vec![],
        result: Some(AnalysedFunctionResult {
            typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Owned),
        })
    }
}

fn get_agent_resource_method_analysed_function(resource_name: String, agent_method_info: &AgentMethodInfo) -> AnalysedFunction {
    let method_name = format!("[method]{}.{}", resource_name, agent_method_info.method_name);

    let mut input_params = vec![AnalysedFunctionParameter {
        name: "agent".to_string(),
        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Owned),
    }];

    let rest_input_params = match &agent_method_info.input_schema {
        Schema::Structured { parameters } => {
            parameters.into_iter().map(|param| {
                AnalysedFunctionParameter {
                    name: "param".to_string(),
                    typ: param.clone(),
                }
            }).collect::<Vec<_>>()
        }
    };

    input_params.extend(rest_input_params);

    let output_params  = match &agent_method_info.output_schema {
        Schema::Structured { parameters } => {
            parameters.first().cloned().map(|x| AnalysedFunctionResult {
                typ: x,
            })
        }
    };

    AnalysedFunction {
        name: method_name,
        parameters: input_params,
        result: output_params,
    }
}

fn get_agent_methods(agent_method_list: &Value, typ: &AnalysedType) -> Vec<AgentMethodInfo> {
    let type_annotated = TypeAnnotatedValue::try_from(ValueAndType::new(agent_method_list.clone(), typ.clone()));

    let agent_methods = type_annotated.unwrap().type_annotated_value.unwrap().to_json_value();

    let agent_methods: Vec<AgentMethodInfo> = serde_json::from_value(agent_methods)
        .expect("Failed to deserialize agent methods");

    agent_methods
}


#[derive(Deserialize, Debug)]
struct AgentMethodInfo {
    #[serde(rename = "prompt-hint")]
    prompt_hint: Option<String>,
    description: String,
    #[serde(rename = "name")]
    method_name: String,
    #[serde(rename = "input-schema")]
    input_schema: Schema,
    #[serde(rename = "output-schema")]
    output_schema: Schema,
}

#[derive(Debug)]
enum Schema {
    Structured {
        parameters: Vec<AnalysedType>,
    },
}

pub struct StructuredParams {
    pub parameters: Parameters
}

pub struct Parameters(pub Vec<AnalysedType>);

impl<'de> Deserialize<'de> for Schema {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        let structured = value
            .get("structured")
            .ok_or_else(|| serde::de::Error::custom("Missing 'structured' field"))?;

        let parameters_value = structured
            .get("parameters")
            .ok_or_else(|| serde::de::Error::custom("Missing 'parameters' field"))?;

        let parameters_array = parameters_value
            .as_array()
            .ok_or_else(|| serde::de::Error::custom("'parameters' must be an array"))?;

        let mut analysed_types = Vec::with_capacity(parameters_array.len());

        for param in parameters_array {
            let param_obj = param
                .as_object()
                .ok_or_else(|| serde::de::Error::custom("Each parameter must be an object"))?;

            let wit = param_obj
                .get("wit")
                .ok_or_else(|| serde::de::Error::custom("Missing 'wit' field"))?;

            let wit_obj = wit
                .as_object()
                .ok_or_else(|| serde::de::Error::custom("'wit' must be an object"))?;

            let nodes = wit_obj
                .get("nodes")
                .ok_or_else(|| serde::de::Error::custom("Missing 'nodes' field"))?;

            let nodes_array = nodes
                .as_array()
                .ok_or_else(|| serde::de::Error::custom("'nodes' must be an array"))?;

            let mut input_wit_nodes = Vec::with_capacity(nodes_array.len());

            for node in nodes_array {
                let node_map = node
                    .as_object()
                    .ok_or_else(|| serde::de::Error::custom("Each node must be an object"))?;

                for (k, _) in node_map {
                    if k == "prim-string-type" {
                        input_wit_nodes.push(WitTypeNode::PrimStringType);
                    }
                }
            }

            let wit_type = WitType {
                nodes: input_wit_nodes,
            };

            analysed_types.push(AnalysedType::from(wit_type));
        }

        Ok(Schema::Structured {
            parameters: analysed_types,
        })
    }
}