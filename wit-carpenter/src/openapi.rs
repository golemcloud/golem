

use std::{collections::BTreeMap, sync::Arc};

use roas::v3_0::{schema::Schema, spec::Spec};
use crate::wit;


pub struct WitOpenApiWrapper{
    spec: Spec,
    types: BTreeMap<String, wit::Type>,
}

pub trait OpenApiToolset {
    fn get_interfaces(&self) -> Vec<wit::Interface>;
    fn get_type(&self, schema: &Schema, required: bool) ->  wit::Type;
    fn init(interfaces: &mut BTreeMap<String, wit::Interface>);
}


impl From<Spec> for wit::Wit {
    fn from(spec: Spec) -> Self {
        let mut wit_openapi_wrapper = WitOpenApiWrapper {
            spec,
            types: BTreeMap::new(),
        };
        let interfaces = WitOpenApiWrapper::get_interfaces(&mut wit_openapi_wrapper);
        let mut exports = vec!["types".to_owned()]; // interface with all types
        exports.extend(
            interfaces.clone().iter().map(|interface| {
                interface.name.clone()
            })
        );

        Self {
            package_meta: wit::PackageMetadata {
                ..Default::default()
            },
            interfaces: interfaces.to_vec(),
            world: wit::World {
            // iter interface and get name for exports
            exports,
            ..Default::default()
            }
        }
    }
}

// impl Toolset for WitCarpenter
impl OpenApiToolset for WitOpenApiWrapper {
    fn get_interfaces(&self) -> Vec<wit::Interface> {
        let mut interfaces = BTreeMap::new();

        Self::init(&mut interfaces);
        
        // 1. OpenAPI Spec -> Paths -> Interfaces
        for (path, path_item) in &self.spec.paths {

            let interface_name = path.split("/").next().unwrap_or_default();

            let interface = interfaces.get_mut(interface_name).or_else(|| {
                 Some(&mut wit::Interface {
                    uses: vec![],
                    resources: vec![],
                    docs: "Interface for path - ".to_string() + path,
                    name: interface_name.to_string(),
                    functions: vec![],
                    types: vec![],
                })
            }).unwrap();

            let resource  = wit::Resource {
                constructor: wit::Constructor {
                    name: interface_name.to_string(),
                    parameters: vec![
                        wit::Parameter{
                            name: "configuration".to_string(),
                            type_name: "configuration".to_string(),
                        }
                    ]
                },
                name: interface_name.to_string(),
                docs: "Resource for path - ".to_string() + path,
                functions: vec![],
            };

            // 2. Path Operations -> Functions 
            for (operation_name, operation) in path_item.operations.expect("operations cannot be empty") {
                let operation_id = operation.operation_id.expect("operation_id cannot be empty").to_string();
                let function = wit::Function {
                    name: operation_id,
                    docs: format!("{}\n{}\n{}", operation_id, operation_name, operation.description.unwrap_or_default()),
                    parameters: vec![],
                    result: None,
                };

                // 3. Parameters/Bodies -> Function Parameters
                let params = get_normal_parameters(operation.parameters); // this include path, query parameters
                let body_param = get_request_body_paramter(operation.request_body);
                function.parameters.extend(params);
                function.parameters.extend(body_params);
                
                // 4. Responses -> Success/Error Types
                let (success_type, error_type) = get_responses_types(operation.responses);
                function.result = Some(wit::ReturnType {
                    ok: success_type,
                    err: error_type
                });
                
                resource.functions.push(function);
            }

            interface.resources.push(resource);
            interfaces.insert(interface_name.to_string(),interface.clone());
        
        }
        interfaces.into_values().collect()
    }

    fn init(interfaces: &mut BTreeMap<String, wit::Interface>) {
        let mut types = vec![];
        // Create global types interface
        let types_interface = wit::Interface {
            uses: vec![],
            resources: vec![],
            docs: "Global types interface".to_string(),
            name: "types".to_string(),
            functions: vec![],
            types: vec![wit::Type{
            name: "Configuration".to_string(),
            docs: Some("Global api configuration".to_string()),
            is_record: true,
            kind: "record".to_string(),
            fields: vec![
                wit::Field {
                    name: "base_url".to_string(),
                    type_name: "String".to_string(),
                },
                // Todo: add more fields later todo!()
            ],
            ..Default::default()
            }],
        };
        
        interfaces.insert("types".to_string(), types_interface);
    }
    
    fn get_type(&self, schema: &Schema, required: bool) ->  wit::Type {
        todo!()
    }
}