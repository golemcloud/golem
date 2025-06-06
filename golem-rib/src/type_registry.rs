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
use crate::{
    DynamicParsedFunctionName, Expr, FullyQualifiedInterfaceName, FunctionDictionary, FunctionName,
    FunctionType, InstanceCreationType, InterfaceName, PackageName, TypeParameter,
};
use golem_wasm_ast::analysis::{AnalysedExport, TypeVariant};
use golem_wasm_ast::analysis::{AnalysedType, TypeEnum};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::{format, Display, Formatter};
use uuid::Uuid;

#[derive(Debug, Default, Hash, Clone, Eq, PartialEq, PartialOrd, Ord)]
pub struct ComponentDependencies {
    pub dependencies: BTreeMap<ComponentInfo, FunctionDictionary>,
}

impl ComponentDependencies {
    pub fn size(&self) -> usize {
        self.dependencies.len()
    }

    pub fn get_variants(&self) -> Vec<TypeVariant> {
        let mut variants = vec![];

        for function_dict in self.dependencies.values() {
            variants.extend(function_dict.get_all_variants());
        }

        variants
    }

    pub fn get_enums(&self) -> Vec<TypeEnum> {
        let mut enums = vec![];

        for function_dict in self.dependencies.values() {
            enums.extend(function_dict.get_all_enums());
        }

        enums
    }

    pub fn get_function_type(
        &self,
        component_info: &Option<ComponentInfo>,
        function_name: &FunctionName,
    ) -> Result<FunctionType, String> {
        // If function name is unique across all components, we are not in need of a component_info per se
        // and we can return the exact component dependency

        dbg!(&self.dependencies);

        match component_info {
            None => {
                let mut function_types_in_component = vec![];

                for (component_info, function_dict) in &self.dependencies {
                    let types = function_dict
                        .name_and_types
                        .iter()
                        .filter_map(|(f_name, function_type)| {
                            if (f_name == function_name) { Some(function_type) } else { None }
                        }).collect::<Vec<_>>();

                    function_types_in_component.push((component_info.clone(), types));
                }

                if function_types_in_component.is_empty() {
                    Err(format!(
                        "function `{}` not found in any component",
                        function_name
                    ))
                } else {
                    if function_types_in_component.len() > 1 {
                        Err(format!(
                            "function `{}` is ambiguous across components",
                            function_name
                        ))
                    } else {
                        Ok(function_types_in_component[0].1[0].clone())
                    }
                }
            }
            Some(component_info) => {
                let function_dictionary = self
                    .dependencies
                    .get(component_info)
                    .cloned()
                    .ok_or_else(|| {
                        format!(
                            "component dependency for `{}` not found",
                            component_info.component_name
                        )
                    })?;

                let function_type = function_dictionary.name_and_types.iter().find_map(
                    |(f_name, function_type)| {
                        if f_name == f_name {
                            Some(function_type.clone())
                        } else {
                            None
                        }
                    },
                );

                if let Some(function_type) = function_type {
                    Ok(function_type)
                } else {
                    Err(format!(
                        "function `{}` not found in component `{}`",
                        function_name, component_info.component_name
                    ))
                }
            }
        }
    }

    pub fn function_dictionary(&self) -> Vec<&FunctionDictionary> {
        self.dependencies.values().collect::<Vec<_>>()
    }

    pub fn filter_by_interface(
        &self,
        interface_name: &InterfaceName,
    ) -> Result<ComponentDependencies, String> {
        let mut tree = BTreeMap::new();

        for (component_info, function_dict) in self.dependencies.iter() {
            let name_and_types: Vec<&(FunctionName, FunctionType)> = function_dict
                .name_and_types
                .iter()
                .filter(|(f, _)| f.interface_name().as_ref() == Some(interface_name))
                .collect::<Vec<_>>();

            if !name_and_types.is_empty() {
                tree.insert(
                    component_info.clone(),
                    FunctionDictionary {
                        name_and_types: name_and_types.into_iter().cloned().collect(),
                    },
                );
            }
        }

        if tree.is_empty() {
            return Err(format!("interface `{}` not found", interface_name));
        }

        Ok(ComponentDependencies { dependencies: tree })
    }

    pub fn filter_by_package_name(
        &self,
        package_name: &PackageName,
    ) -> Result<ComponentDependencies, String> {
        // If the package name corresponds to the root package name we pick that up
        let mut tree = BTreeMap::new();

        for (component_info, function_dict) in self.dependencies.iter() {
            if let Some(root_package_name) = &component_info.root_package_name {
                if root_package_name == &package_name.to_string() {
                    tree.insert(component_info.clone(), function_dict.clone());
                }
            } else {
                // If this package doesn't correspond to a root, but happens to be part of the component then

                let name_and_types = function_dict
                    .name_and_types
                    .iter()
                    .filter(|(f, _)| f.package_name() == Some(package_name.clone()))
                    .collect::<Vec<_>>();

                if !name_and_types.is_empty() {
                    tree.insert(
                        component_info.clone(),
                        FunctionDictionary {
                            name_and_types: name_and_types.into_iter().cloned().collect(),
                        },
                    );
                }
            }
        }

        if tree.is_empty() {
            return Err(format!("package `{}` not found", package_name));
        }

        Ok(ComponentDependencies { dependencies: tree })
    }

    pub fn filter_by_fully_qualified_interface(
        &self,
        fqi: &FullyQualifiedInterfaceName,
    ) -> Result<Self, String> {
        let mut tree = BTreeMap::new();

        for (component_info, function_dict) in self.dependencies.iter() {
            if let Some(root_package_name) = &component_info.root_package_name {
                if root_package_name == &fqi.package_name.to_string() {
                    tree.insert(component_info.clone(), function_dict.clone());
                }
            } else {
                // If this package doesn't correspond to a root, but happens to be part of the component then

                let name_and_types = function_dict
                    .name_and_types
                    .iter()
                    .filter(|(f, _)| {
                        f.package_name() == Some(fqi.package_name.clone())
                            && f.interface_name() == Some(fqi.interface_name.clone())
                    })
                    .collect::<Vec<_>>();

                if !name_and_types.is_empty() {
                    tree.insert(
                        component_info.clone(),
                        FunctionDictionary {
                            name_and_types: name_and_types.into_iter().cloned().collect(),
                        },
                    );
                }
            }
        }

        if tree.is_empty() {
            return Err(format!("fully qualified interface `{}` not found", fqi));
        }

        Ok(ComponentDependencies { dependencies: tree })
    }

    // type-parameter can be None.
    // If present, it may represent the root package name of the component
    // or it could represent the package or interface within a component
    pub fn get_worker_instance_type(
        &self,
        type_parameter: Option<TypeParameter>,
        worker_name: Option<Expr>,
    ) -> Result<InstanceCreationType, String> {
        match type_parameter {
            None => Ok(InstanceCreationType::Worker {
                component_info: None,
                worker_name: worker_name.map(|expr| Box::new(expr)),
            }),

            Some(type_parameter) => {
                match type_parameter {
                    // If the user has specified the root package name, annotate the InstanceCreationType with the component already
                    TypeParameter::PackageName(package_name) => {
                        let result =
                            self.dependencies
                                .iter()
                                .find(|(x, y)| match &x.root_package_name {
                                    Some(name) => {
                                        let pkg = match &x.root_package_version {
                                            None => name.to_string(),
                                            Some(version) => format!("{}@{}", name, version),
                                        };

                                        pkg == package_name.to_string()
                                    }

                                    None => false,
                                });

                        if let Some(result) = result {
                            Ok(InstanceCreationType::Worker {
                                component_info: Some(result.0.clone()),
                                worker_name: worker_name.map(|expr| Box::new(expr)),
                            })
                        } else {
                            Ok(InstanceCreationType::Worker {
                                component_info: None,
                                worker_name: worker_name.map(|expr| Box::new(expr)),
                            })
                        }
                    }

                    _ => Ok(InstanceCreationType::Worker {
                        component_info: None,
                        worker_name: worker_name.map(|expr| Box::new(expr)),
                    }),
                }
            }
        }
    }

    pub fn from_raw(
        component_and_exports: Vec<(ComponentInfo, &Vec<AnalysedExport>)>,
    ) -> Result<Self, String> {
        let mut dependencies = BTreeMap::new();

        for (component_info, exports) in component_and_exports {
            let function_type_registry = FunctionTypeRegistry::from_export_metadata(exports);
            let function_dictionary =
                FunctionDictionary::from_function_type_registry(&function_type_registry)?;
            dependencies.insert(component_info, function_dictionary);
        }

        Ok(ComponentDependencies { dependencies })
    }
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, Ord, PartialOrd)]
pub struct ComponentInfo {
    pub component_name: String,
    pub component_id: Uuid,
    pub root_package_name: Option<String>,
    pub root_package_version: Option<String>,
}

impl Display for ComponentInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Component: {}, ID: {}, Root Package: {}@{}",
            self.component_name,
            self.component_id,
            self.root_package_name.as_deref().unwrap_or("unknown"),
            self.root_package_version.as_deref().unwrap_or("unknown")
        )
    }
}

// A type-registry is a mapping from a function/variant/enum to the `arguments` and `return types` of that function/variant/enum.
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
            RegistryKey::FunctionName(name) => write!(f, "Function Name: {}", name),
            RegistryKey::FunctionNameWithInterface {
                interface_name,
                function_name,
            } => write!(
                f,
                "Interface: {}, Function: {}",
                interface_name, function_name
            ),
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
                format!["[constructor]{}", resource_name_without_prefix];

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
            }) => {
                update_registry(ok_type.as_ref(), registry);
                update_registry(err_type.as_ref(), registry);
            }
            AnalysedType::Result(TypeResult {
                ok: None,
                err: Some(err_type),
            }) => {
                update_registry(err_type.as_ref(), registry);
            }
            AnalysedType::Result(TypeResult {
                ok: Some(ok_type),
                err: None,
            }) => {
                update_registry(ok_type.as_ref(), registry);
            }
            AnalysedType::Option(type_option) => {
                update_registry(type_option.inner.as_ref(), registry);
            }
            AnalysedType::Result(TypeResult {
                ok: None,
                err: None,
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

#[cfg(feature = "protobuf")]
mod protobuf {
    use crate::RegistryKey;
    use golem_api_grpc::proto::golem::rib::registry_key::KeyType;

    impl TryFrom<golem_api_grpc::proto::golem::rib::RegistryKey> for RegistryKey {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::rib::RegistryKey,
        ) -> Result<Self, Self::Error> {
            let key_type = value.key_type.ok_or("key type missing")?;

            let registry_key = match key_type {
                KeyType::FunctionName(string) => RegistryKey::FunctionName(string.name),
                KeyType::FunctionNameWithInterface(function_with_interface) => {
                    let interface_name = function_with_interface.interface_name.clone();
                    let function_name = function_with_interface.function_name;

                    RegistryKey::FunctionNameWithInterface {
                        interface_name,
                        function_name,
                    }
                }
            };

            Ok(registry_key)
        }
    }

    impl From<&RegistryKey> for golem_api_grpc::proto::golem::rib::RegistryKey {
        fn from(value: &RegistryKey) -> Self {
            match value {
                RegistryKey::FunctionName(name) => golem_api_grpc::proto::golem::rib::RegistryKey {
                    key_type: Some(KeyType::FunctionName(
                        golem_api_grpc::proto::golem::rib::FunctionName {
                            name: name.to_string(),
                        },
                    )),
                },
                RegistryKey::FunctionNameWithInterface {
                    function_name,
                    interface_name,
                } => golem_api_grpc::proto::golem::rib::RegistryKey {
                    key_type: Some(KeyType::FunctionNameWithInterface(
                        golem_api_grpc::proto::golem::rib::FunctionNameWithInterface {
                            interface_name: interface_name.clone(),
                            function_name: function_name.clone(),
                        },
                    )),
                },
            }
        }
    }
}
