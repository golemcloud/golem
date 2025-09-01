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

use crate::call_type::CallType;
use crate::DynamicParsedFunctionName;
use golem_wasm_ast::analysis::{AnalysedExport, TypeVariant};
use golem_wasm_ast::analysis::{AnalysedType, TypeEnum};
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};

// A `FunctionTypeRegistry` is a mapping from a function/variant/enum to the `arguments` and `return types` of that function/variant/enum.
// The structure is raw and closer to the original component metadata.
// FunctionTypeRegistry act as a set of all dependencies in Rib.
// Currently, it talks about only 1 component.
#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub struct FunctionTypeRegistry {
    pub types: HashMap<RegistryKey, RegistryValue>,
}

impl FunctionTypeRegistry {
    pub fn get_from_keys(&self, keys: HashSet<RegistryKey>) -> FunctionTypeRegistry {
        let mut types = HashMap::new();
        for key in keys {
            let registry_value = self.lookup(&key);
            if let Some(registry_value) = registry_value {
                types.insert(key, registry_value);
            }
        }

        FunctionTypeRegistry { types }
    }

    pub fn get_variants(&self) -> Vec<TypeVariant> {
        let mut variants = vec![];

        for registry_value in self.types.values() {
            if let RegistryValue::Variant { variant_type, .. } = registry_value {
                variants.push(variant_type.clone())
            }
        }

        variants
    }

    pub fn get_enums(&self) -> Vec<TypeEnum> {
        let mut enums = vec![];

        for registry_value in self.types.values() {
            if let RegistryValue::Value(AnalysedType::Enum(type_enum)) = registry_value {
                enums.push(type_enum.clone())
            }
        }

        enums
    }

    pub fn get(&self, key: &CallType) -> Option<&RegistryValue> {
        match key {
            CallType::Function { function_name, .. } => self
                .types
                .get(&RegistryKey::fqn_registry_key(function_name)),
            CallType::VariantConstructor(variant_name) => self
                .types
                .get(&RegistryKey::FunctionName(variant_name.clone())),
            CallType::EnumConstructor(enum_name) => self
                .types
                .get(&RegistryKey::FunctionName(enum_name.clone())),
            CallType::InstanceCreation(_) => None,
        }
    }

    pub fn empty() -> Self {
        Self {
            types: HashMap::new(),
        }
    }

    pub fn from_export_metadata(exports: &[AnalysedExport]) -> Self {
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

                        let return_type = fun.result.map(|result| {
                            let analysed_type = result.typ;
                            types.insert(analysed_type.clone());
                            analysed_type
                        });

                        let registry_key = RegistryKey::FunctionNameWithInterface {
                            interface_name: interface_name.clone(),
                            function_name: function_name.clone(),
                        };

                        let registry_value = RegistryValue::Function {
                            parameter_types,
                            return_type,
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

                    let return_type = fun.result.map(|result| {
                        let analysed_type = result.typ;
                        types.insert(analysed_type.clone());
                        analysed_type
                    });

                    let registry_value = RegistryValue::Function {
                        parameter_types,
                        return_type,
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

// A registry key in Rib can in include real functions including the variant constructors.
#[derive(Hash, Eq, PartialEq, PartialOrd, Ord, Clone, Debug)]
pub enum RegistryKey {
    FunctionName(String),
    FunctionNameWithInterface {
        interface_name: String,
        function_name: String,
    },
}

impl Display for RegistryKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryKey::FunctionName(name) => write!(f, "Function Name: {name}"),
            RegistryKey::FunctionNameWithInterface {
                interface_name,
                function_name,
            } => write!(f, "Interface: {interface_name}, Function: {function_name}"),
        }
    }
}

impl RegistryKey {
    // Get the function name without the interface
    // Note that this function name can be the name of the resource constructor,
    // or resource method, or simple function name, that correspond to the real
    // component metadata. Examples:
    // [constructor]shopping-cart,
    // [method]add-to-cart,
    // checkout
    pub fn get_function_name(&self) -> String {
        match self {
            Self::FunctionName(str) => str.clone(),
            Self::FunctionNameWithInterface { function_name, .. } => function_name.clone(),
        }
    }

    pub fn get_interface_name(&self) -> Option<String> {
        match self {
            Self::FunctionName(_) => None,
            Self::FunctionNameWithInterface { interface_name, .. } => Some(interface_name.clone()),
        }
    }

    // A parsed function name (the one that gets invoked with a worker) can correspond
    // to multiple registry keys. For example: this is mainly because a function can have a constructor component
    // along with the method name (2 registry keys correspond to this 1 function).
    // Otherwise it's only 1 key that correspond to the Fqn always.
    pub fn registry_keys_of_function(
        function_name: &DynamicParsedFunctionName,
    ) -> Vec<RegistryKey> {
        let resource_constructor_key = Self::resource_constructor_registry_key(function_name);
        let function_name_registry_key = Self::fqn_registry_key(function_name);
        if let Some(resource_constructor_key) = resource_constructor_key {
            vec![resource_constructor_key, function_name_registry_key]
        } else {
            vec![function_name_registry_key]
        }
    }

    // To obtain the registry key that correspond to the FQN of the function
    // Note that, it will not provide the registry key corresponding to the constructor of a resource
    // if the function was part of a resource
    pub fn fqn_registry_key(function: &DynamicParsedFunctionName) -> RegistryKey {
        let resource_method_name_in_metadata = function.function_name_with_prefix_identifiers();

        match function.site.interface_name() {
            None => RegistryKey::FunctionName(resource_method_name_in_metadata),
            Some(interface) => RegistryKey::FunctionNameWithInterface {
                interface_name: interface.to_string(),
                function_name: resource_method_name_in_metadata,
            },
        }
    }

    // Obtain the registry-key corresponding to the resource constructor in a dynamic parsed function name
    pub fn resource_constructor_registry_key(
        function: &DynamicParsedFunctionName,
    ) -> Option<RegistryKey> {
        let resource_name_without_prefixes = function.resource_name_simplified();

        resource_name_without_prefixes.map(|resource_name_without_prefix| {
            let resource_constructor_with_prefix =
                format!["[constructor]{resource_name_without_prefix}"];

            match function.site.interface_name() {
                None => RegistryKey::FunctionName(resource_constructor_with_prefix),
                Some(interface) => RegistryKey::FunctionNameWithInterface {
                    interface_name: interface.to_string(),
                    function_name: resource_constructor_with_prefix,
                },
            }
        })
    }
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum RegistryValue {
    Value(AnalysedType),
    Variant {
        parameter_types: Vec<AnalysedType>,
        variant_type: TypeVariant,
    },
    Function {
        parameter_types: Vec<AnalysedType>,
        return_type: Option<AnalysedType>,
    },
}

impl RegistryValue {
    pub fn argument_types(&self) -> Vec<AnalysedType> {
        match self {
            RegistryValue::Function {
                parameter_types,
                return_type: _,
            } => parameter_types.clone(),
            RegistryValue::Variant {
                parameter_types,
                variant_type: _,
            } => parameter_types.clone(),
            RegistryValue::Value(_) => vec![],
        }
    }
}

mod internal {
    use crate::{RegistryKey, RegistryValue};
    use golem_wasm_ast::analysis::{AnalysedType, TypeResult};
    use std::collections::HashMap;

    pub(crate) fn update_registry(
        ty: &AnalysedType,
        registry: &mut HashMap<RegistryKey, RegistryValue>,
    ) {
        match ty.clone() {
            AnalysedType::Variant(variant) => {
                let type_variant = variant.clone();
                for name_type_pair in &type_variant.cases {
                    registry.insert(RegistryKey::FunctionName(name_type_pair.name.clone()), {
                        name_type_pair.typ.clone().map_or(
                            RegistryValue::Value(ty.clone()),
                            |variant_parameter_typ| RegistryValue::Variant {
                                parameter_types: vec![variant_parameter_typ],
                                variant_type: type_variant.clone(),
                            },
                        )
                    });
                }
            }

            AnalysedType::Enum(type_enum) => {
                for name_type_pair in type_enum.cases {
                    registry.insert(
                        RegistryKey::FunctionName(name_type_pair.clone()),
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

            AnalysedType::Result(TypeResult {
                ok: Some(ok_type),
                err: Some(err_type),
                ..
            }) => {
                update_registry(ok_type.as_ref(), registry);
                update_registry(err_type.as_ref(), registry);
            }
            AnalysedType::Result(TypeResult {
                ok: None,
                err: Some(err_type),
                ..
            }) => {
                update_registry(err_type.as_ref(), registry);
            }
            AnalysedType::Result(TypeResult {
                ok: Some(ok_type),
                err: None,
                ..
            }) => {
                update_registry(ok_type.as_ref(), registry);
            }
            AnalysedType::Option(type_option) => {
                update_registry(type_option.inner.as_ref(), registry);
            }
            AnalysedType::Result(TypeResult {
                ok: None,
                err: None,
                ..
            }) => {}
            AnalysedType::Flags(_) => {}
            AnalysedType::Str(_) => {}
            AnalysedType::Chr(_) => {}
            AnalysedType::F64(_) => {}
            AnalysedType::F32(_) => {}
            AnalysedType::U64(_) => {}
            AnalysedType::S64(_) => {}
            AnalysedType::U32(_) => {}
            AnalysedType::S32(_) => {}
            AnalysedType::U16(_) => {}
            AnalysedType::S16(_) => {}
            AnalysedType::U8(_) => {}
            AnalysedType::S8(_) => {}
            AnalysedType::Bool(_) => {}
            AnalysedType::Handle(_) => {}
        }
    }
}
