// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::call_type::CallType;
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
    EnumName(String),
    FunctionName(String),
    FunctionNameWithInterface {
        interface_name: String,
        function_name: String,
    },
}

impl RegistryKey {
    pub fn from_invocation_name(invocation_name: &CallType) -> RegistryKey {
        match invocation_name {
            CallType::VariantConstructor(variant_name) => {
                RegistryKey::VariantName(variant_name.clone())
            }
            CallType::EnumConstructor(enum_name) => RegistryKey::EnumName(enum_name.clone()),
            CallType::Function(function_name) => match function_name.site().interface_name() {
                None => RegistryKey::FunctionName(function_name.function().function_name()),
                Some(interface_name) => RegistryKey::FunctionNameWithInterface {
                    interface_name: interface_name.to_string(),
                    function_name: function_name.function().function_name(),
                },
            },
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
            internal::update_registry(&ty, &mut map);
        }

        Self { types: map }
    }

    pub fn lookup(&self, registry_key: &RegistryKey) -> Option<RegistryValue> {
        self.types.get(registry_key).cloned()
    }
}

mod internal {
    use crate::{RegistryKey, RegistryValue};
    use golem_wasm_ast::analysis::AnalysedType;
    use std::collections::HashMap;

    pub(crate) fn update_registry(
        ty: &AnalysedType,
        registry: &mut HashMap<RegistryKey, RegistryValue>,
    ) {
        match ty.clone() {
            AnalysedType::Variant(variant) => {
                for name_type_pair in variant.cases {
                    registry.insert(RegistryKey::VariantName(name_type_pair.name.clone()), {
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

            AnalysedType::Enum(type_enum) => {
                for name_type_pair in type_enum.cases {
                    registry.insert(
                        RegistryKey::EnumName(name_type_pair.clone()),
                        RegistryValue::Value(ty.clone()),
                    );
                }
            }

            AnalysedType::Tuple(tuple) => {
                for element in tuple.items {
                    update_registry(&element, registry);
                }
            }

            AnalysedType::List(list) => {
                update_registry(list.inner.as_ref(), registry);
            }

            AnalysedType::Record(record) => {
                for name_type in record.fields.iter() {
                    update_registry(&name_type.typ, registry);
                }
            }

            _ => {}
        }
    }
}
