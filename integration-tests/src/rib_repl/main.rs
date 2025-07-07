use golem_rib_repl::{ComponentSource, RibRepl, RibReplConfig};
use golem_test_framework::config::{
    EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, TestDependencies,
};
use integration_tests::rib_repl::bootstrap::*;
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use golem_client::model::AnalysedType;
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

    let mut rib_repl = RibRepl::bootstrap(RibReplConfig {
        history_file: None,
        dependency_manager: Arc::new(TestRibReplDependencyManager::new(deps.clone())),
        worker_function_invoke: Arc::new(TestRibReplWorkerFunctionInvoke::new(deps.clone())),
        printer: None,
        component_source: Some(ComponentSource {
            component_name: component_name.to_string(),
            source_path: deps
                .component_directory()
                .join(format!("{component_name}.wasm")),
        }),
        prompt: None,
        command_registry: None,
    })
    .await
    .expect("Failed to bootstrap REPL");

    rib_repl.execute(
        "let x = instance()"
    ).await.expect("Failed to execute command");

    let result = rib_repl.execute(
        "x.discover-agent-definitions()"
    ).await.expect("Failed to execute command").unwrap();

    match result  {
        RibResult::Val(value_and_type) => {
           match value_and_type.value {
               Value::List(values) => {
                   let typ = value_and_type.typ;
                   match typ {
                       AnalysedType::List(typed_list) => {
                           match typed_list.inner.as_ref() {
                               AnalysedType::Record(typed_record) => {
                                   let record = typed_record.fields.iter().enumerate().find(
                                       |(_, field)| field.name == "methods"
                                   );

                                   let (index, name_type) =
                                       record.expect("Expected 'methods' field in record");

                                   // Each value is an agent
                                   for agent in values {
                                       match agent {
                                             Value::Record(values) => {
                                                  let methods = values.get(index)
                                                    .expect("Expected value for 'methods' field");

                                                  get_from_agent_methods(methods, &name_type.typ);
                                             }

                                             _ => {
                                                  panic!("Expected a record type, got: {:?}", agent);
                                             }
                                       }
                                   }

                               }

                               _ => {
                                   panic!("Expected a record type, got: {:?}", typed_list.inner);
                               }
                           }
                       }

                          _ => panic!("Expected a list type, got: {:?}", typ),
                   }

               }

               _ => {}
           }

        }

        _ => panic!("Expected a value result, got: {:?}", result),
    }



    rib_repl.run().await
}

fn get_from_agent_methods(record: &Value, typ: &AnalysedType) -> Vec<AgentMethodInfo> {
    let type_annotated = TypeAnnotatedValue::try_from(ValueAndType::new(record.clone(), typ.clone()));

    let agent_methods = type_annotated.unwrap().type_annotated_value.unwrap().to_json_value();

    let agent_methods: Vec<AgentMethodInfo> = serde_json::from_value(agent_methods)
        .expect("Failed to deserialize agent methods");

    dbg!(&agent_methods);
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