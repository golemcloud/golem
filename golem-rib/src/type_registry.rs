use std::collections::{HashMap, HashSet};
use golem_wasm_ast::analysis::AnalysedType;
use golem_service_base::model::{ComponentMetadata, Export, Type};

#[derive(Hash, PartialEq, Clone, Debug)]
pub enum RegistryKey {
    FunctionName(String),
    FunctionNameWithInterface {
        interface_name : String,
        function_name: String,
    }
}

// TODO; Add Hash to AnalysedType
#[derive(Hash, PartialEq, Clone, Debug)]
pub enum RegistryValue {
    Value(AnalysedType),
    Function {
        parameter_types: Vec<AnalysedType>,
        return_types: Vec<AnalysedType>
    }
}


#[derive(Hash, PartialEq, Clone, Debug)]
pub struct FunctionTypeRegistry {
    types: HashMap<RegistryKey, Vec<RegistryValue>>,
}

impl FunctionTypeRegistry {

    pub fn analyse(component_metadata: &ComponentMetadata) -> Self {
        let exports = &component_metadata.exports;

        let mut map = HashMap::new();

        let mut types = HashSet::new();

        for export in exports {
            match export {
                Export::Instance(ty) => {
                    let interface_name = &ty.name;
                    for fun in ty.functions {
                        let function_name = fun.name;
                        let parameter_types = fun.parameters.into_iter().map(|parameter| {
                            let analysed_type = AnalysedType::from(parameter.typ);
                            types.insert(analysed_type.clone());
                            analysed_type

                        }).collect::<Vec<_>>();

                        let return_types = fun.results.into_iter().map(|result| {
                            let analysed_type = AnalysedType::from(result.typ);
                            types.insert(analysed_type.clone());
                            analysed_type
                        }).collect::<Vec<_>>();

                       let registry_value = RegistryValue::Function {
                            parameter_types,
                            return_types
                       };

                        let registry_key = RegistryKey::FunctionNameWithInterface {
                            interface_name: interface_name.clone(),
                            function_name: function_name.clone()
                        };

                        map.entry(registry_key).or_insert_with(Vec::new).push(registry_value);
                    }
                }
                Export::Function(fun0) => {
                    let fun = fun0.clone();
                    let function_name = fun.name;
                    let parameter_types = fun.parameters.into_iter().map(|parameter| {
                        let analysed_type = AnalysedType::from(parameter.typ);
                        types.insert(analysed_type.clone());
                        analysed_type
                    }).collect::<Vec<_>>();

                    let return_types = fun.results.into_iter().map(|result| {
                        let analysed_type = AnalysedType::from(result.typ);
                        types.insert(analysed_type.clone());
                        analysed_type
                    }).collect::<Vec<_>>();

                    let registry_value = RegistryValue::Function {
                        parameter_types,
                        return_types
                    };

                    let registry_key = RegistryKey::FunctionName(function_name.clone());

                    map.entry(registry_key).or_insert_with(Vec::new).push(registry_value);

                }
            }
        }

        for ty in types {
            match ty.clone() {
                AnalysedType::Variant(variant) =>
                    for (field, variant_param_typ) in variant {
                        map.entry(RegistryKey::FunctionName(field.clone())).or_insert_with(Vec::new).push({
                            variant_param_typ.map_or(RegistryValue::Value(ty.clone()), |variant_parameter_typ| {
                                RegistryValue::Function {
                                    parameter_types: vec![variant_parameter_typ],
                                    return_types: vec![ty]
                                }
                            })
                        });
                    },
                _ => {}
            }
        }

        Self {
            types: map
        }
    }

    pub fn lookup(&self, registry_key: &RegistryKey) -> Option<Vec<RegistryValue>> {
        self.types.get(registry_key).cloned()
    }

}