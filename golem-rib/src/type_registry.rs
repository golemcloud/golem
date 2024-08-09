use golem_wasm_ast::analysis::AnalysedExport;
use golem_wasm_ast::analysis::AnalysedType;
use std::collections::{HashMap, HashSet};

// A type-registry is a mapping from a function name (global or part of an interface in WIT)
// to the registry value that represents the type of the name.
// Here, registry key names are called function names (and not really the names of the types),
// as this is what the component-model parser output (golem-wasm-ast) gives us.
// We make sure if we bump into any variant types (as part of processing the function parameter types),
// we store them as a mapping from FunctionName(name_of_variant) to a registry value. If the variant
// has parameters, then the RegistryValue is considered a function type itself with parameter types,
// and a return type that the member variant represents. If the variant has no parameters,
// then the RegistryValue is simply an AnalysedType representing the variant type itself.
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub enum RegistryKey {
    FunctionName(String),
    FunctionNameWithInterface {
        interface_name: String,
        function_name: String,
    },
}

#[derive(PartialEq, Clone, Debug)]
pub enum RegistryValue {
    Value(AnalysedType),
    Function {
        parameter_types: Vec<AnalysedType>,
        return_types: Vec<AnalysedType>,
    },
}

#[derive(Clone, Debug)]
pub struct FunctionTypeRegistry {
    pub types: HashMap<RegistryKey, Vec<RegistryValue>>,
}

impl FunctionTypeRegistry {
    pub fn analyse(exports: &Vec<AnalysedExport>) -> Self {
        let mut map = HashMap::new();

        let mut types = HashSet::new();

        for export in exports {
            match export {
                AnalysedExport::Instance(ty) => {
                    let interface_name = &ty.name;
                    for fun in ty.functions {
                        let function_name = fun.name;
                        let parameter_types = fun
                            .parameters
                            .into_iter()
                            .map(|parameter| {
                                let analysed_type = AnalysedType::from(parameter.typ);
                                types.insert(analysed_type.clone());
                                analysed_type
                            })
                            .collect::<Vec<_>>();

                        let return_types = fun
                            .results
                            .into_iter()
                            .map(|result| {
                                let analysed_type = AnalysedType::from(result.typ);
                                types.insert(analysed_type.clone());
                                analysed_type
                            })
                            .collect::<Vec<_>>();

                        let registry_value = RegistryValue::Function {
                            parameter_types,
                            return_types,
                        };

                        let registry_key = RegistryKey::FunctionNameWithInterface {
                            interface_name: interface_name.clone(),
                            function_name: function_name.clone(),
                        };

                        map.entry(registry_key)
                            .or_insert_with(Vec::new)
                            .push(registry_value);
                    }
                }
                AnalysedExport::Function(fun0) => {
                    let fun = fun0.clone();
                    let function_name = fun.name;
                    let parameter_types = fun
                        .parameters
                        .into_iter()
                        .map(|parameter| {
                            let analysed_type = AnalysedType::from(parameter.typ);
                            types.insert(analysed_type.clone());
                            analysed_type
                        })
                        .collect::<Vec<_>>();

                    let return_types = fun
                        .results
                        .into_iter()
                        .map(|result| {
                            let analysed_type = AnalysedType::from(result.typ);
                            types.insert(analysed_type.clone());
                            analysed_type
                        })
                        .collect::<Vec<_>>();

                    let registry_value = RegistryValue::Function {
                        parameter_types,
                        return_types,
                    };

                    let registry_key = RegistryKey::FunctionName(function_name.clone());

                    map.entry(registry_key)
                        .or_insert_with(Vec::new)
                        .push(registry_value);
                }
            }
        }

        for ty in types {
            match ty.clone() {
                AnalysedType::Variant(variant) => {
                    for (field, variant_param_typ) in variant {
                        map.entry(RegistryKey::FunctionName(field.clone()))
                            .or_insert_with(Vec::new)
                            .push({
                                variant_param_typ.map_or(
                                    RegistryValue::Value(ty.clone()),
                                    |variant_parameter_typ| RegistryValue::Function {
                                        parameter_types: vec![variant_parameter_typ],
                                        return_types: vec![ty],
                                    },
                                )
                            });
                    }
                }
                _ => {}
            }
        }

        Self { types: map }
    }

    pub fn lookup(&self, registry_key: &RegistryKey) -> Option<Vec<RegistryValue>> {
        self.types.get(registry_key).cloned()
    }
}
