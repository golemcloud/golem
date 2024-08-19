use crate::InvocationName;
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
    VariantName(String),
    FunctionName(String),
    FunctionNameWithInterface {
        interface_name: String,
        function_name: String,
    },
}

impl RegistryKey {
    pub fn from_invocation_name(invocation_name: &InvocationName) -> RegistryKey {
        match invocation_name {
            InvocationName::VariantConstructor(variant_name) => {
                RegistryKey::VariantName(variant_name.clone())
            }
            InvocationName::Function(function_name) => {
                match function_name.site().interface_name() {
                    None => RegistryKey::FunctionName(function_name.function().function_name()),
                    Some(interface_name) => RegistryKey::FunctionNameWithInterface {
                        interface_name: interface_name.to_string(),
                        function_name: function_name.function().function_name(),
                    },
                }
            }
        }
    }
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
    pub types: HashMap<RegistryKey, RegistryValue>,
}

impl FunctionTypeRegistry {
    pub fn empty() -> Self {
        Self {
            types: HashMap::new(),
        }
    }

    pub fn from_export_metadata(exports: &Vec<AnalysedExport>) -> Self {
        let mut map = HashMap::new();

        let mut types = HashSet::new();

        for export in exports {
            match export {
                AnalysedExport::Instance(ty) => {
                    let interface_name = &ty.name;
                    for fun in ty.functions.clone() {
                        let function_name = fun.name;
                        let parameter_types = fun
                            .parameters
                            .into_iter()
                            .map(|parameter| {
                                let analysed_type = parameter.typ;
                                types.insert(analysed_type.clone());
                                analysed_type
                            })
                            .collect::<Vec<_>>();

                        let return_types = fun
                            .results
                            .into_iter()
                            .map(|result| {
                                let analysed_type = result.typ;
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

                        map.insert(registry_key, registry_value);
                    }
                }
                AnalysedExport::Function(fun0) => {
                    let fun = fun0.clone();
                    let function_name = fun.name;
                    let parameter_types = fun
                        .parameters
                        .into_iter()
                        .map(|parameter| {
                            let analysed_type = parameter.typ;
                            types.insert(analysed_type.clone());
                            analysed_type
                        })
                        .collect::<Vec<_>>();

                    let return_types = fun
                        .results
                        .into_iter()
                        .map(|result| {
                            let analysed_type = result.typ;
                            types.insert(analysed_type.clone());
                            analysed_type
                        })
                        .collect::<Vec<_>>();

                    let registry_value = RegistryValue::Function {
                        parameter_types,
                        return_types,
                    };

                    let registry_key = RegistryKey::FunctionName(function_name.clone());

                    map.insert(registry_key, registry_value);
                }
            }
        }

        for ty in types {
            if let AnalysedType::Variant(variant) = ty.clone() {
                for name_type_pair in variant.cases {
                    map.insert(RegistryKey::VariantName(name_type_pair.name.clone()), {
                        name_type_pair.typ.map_or(
                            RegistryValue::Value(ty.clone()),
                            |variant_parameter_typ| RegistryValue::Function {
                                parameter_types: vec![variant_parameter_typ],
                                return_types: vec![ty.clone()],
                            },
                        )
                    });
                }
            }
        }

        Self { types: map }
    }

    pub fn lookup(&self, registry_key: &RegistryKey) -> Option<RegistryValue> {
        self.types.get(registry_key).cloned()
    }
}
