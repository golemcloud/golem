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

use crate::{
    ComponentDependencyKey, Expr, FullyQualifiedInterfaceName, FunctionDictionary, FunctionName,
    FunctionType, FunctionTypeRegistry, InstanceCreationType, InterfaceName, PackageName,
    TypeParameter,
};
use golem_wasm_ast::analysis::TypeEnum;
use golem_wasm_ast::analysis::{AnalysedExport, TypeVariant};
use std::collections::BTreeMap;

#[derive(Debug, Default, Hash, Clone, Eq, PartialEq, PartialOrd, Ord)]
pub struct ComponentDependencies {
    pub dependencies: BTreeMap<ComponentDependencyKey, FunctionDictionary>,
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
        component_info: &Option<ComponentDependencyKey>,
        function_name: &FunctionName,
    ) -> Result<(ComponentDependencyKey, FunctionType), String> {
        // If function name is unique across all components, we are not in need of a component_info per se
        // and we can return the exact component dependency
        match component_info {
            None => {
                let mut function_types_in_component = vec![];

                for (component_dependency_key, function_dict) in &self.dependencies {
                    let types = function_dict
                        .name_and_types
                        .iter()
                        .filter_map(|(f_name, function_type)| {
                            if f_name == function_name {
                                Some(function_type)
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>();

                    function_types_in_component.push((component_dependency_key.clone(), types));
                }

                if function_types_in_component.is_empty() {
                    Err("unknown function".to_string())
                } else if function_types_in_component.len() > 1 {
                    Err(format!(
                        "function `{function_name}` is ambiguous across components"
                    ))
                } else {
                    let (key, types) = function_types_in_component.pop().unwrap();

                    if types.is_empty() {
                        Err("unknown function".to_string())
                    } else {
                        Ok((key, types[0].clone()))
                    }
                }
            }
            Some(component_dep_key) => {
                let function_dictionary = self
                    .dependencies
                    .get(component_dep_key)
                    .cloned()
                    .ok_or_else(|| {
                        format!(
                            "component dependency for `{}` not found",
                            component_dep_key.component_name
                        )
                    })?;

                let function_type = function_dictionary.name_and_types.iter().find_map(
                    |(f_name, function_type)| {
                        if f_name == function_name {
                            Some(function_type.clone())
                        } else {
                            None
                        }
                    },
                );

                if let Some(function_type) = function_type {
                    Ok((component_dep_key.clone(), function_type))
                } else {
                    Err(format!(
                        "function `{}` not found in component `{}`",
                        function_name, component_dep_key.component_name
                    ))
                }
            }
        }
    }

    pub fn narrow_to_component(&mut self, component_dependency_key: &ComponentDependencyKey) {
        // If the component dependency key is not found, we do nothing
        if let Some(function_dict) = self.dependencies.remove(component_dependency_key) {
            self.dependencies.clear();
            self.dependencies
                .insert(component_dependency_key.clone(), function_dict);
        }
    }

    pub fn function_dictionary(&self) -> Vec<&FunctionDictionary> {
        self.dependencies.values().collect::<Vec<_>>()
    }

    pub fn filter_by_interface(
        &self,
        interface_name: &InterfaceName,
    ) -> Result<crate::ComponentDependencies, String> {
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
            return Err(format!("interface `{interface_name}` not found"));
        }

        Ok(ComponentDependencies { dependencies: tree })
    }

    pub fn filter_by_package_name(
        &self,
        package_name: &PackageName,
    ) -> Result<crate::ComponentDependencies, String> {
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
            return Err(format!("package `{package_name}` not found"));
        }

        Ok(crate::ComponentDependencies { dependencies: tree })
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
            return Err(format!("`{fqi}` not found"));
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
            None => Ok(InstanceCreationType::WitWorker {
                component_info: None,
                worker_name: worker_name.map(Box::new),
            }),

            Some(TypeParameter::PackageName(package_name)) => {
                // If the user has specified the root package name, annotate the InstanceCreationType with the component already
                let result = self
                    .dependencies
                    .iter()
                    .find(|(x, _)| match &x.root_package_name {
                        Some(name) => {
                            let pkg = match &x.root_package_version {
                                None => name.to_string(),
                                Some(version) => format!("{name}@{version}"),
                            };

                            pkg == package_name.to_string()
                        }

                        None => false,
                    });

                if let Some(result) = result {
                    Ok(InstanceCreationType::WitWorker {
                        component_info: Some(result.0.clone()),
                        worker_name: worker_name.map(Box::new),
                    })
                } else {
                    Ok(InstanceCreationType::WitWorker {
                        component_info: None,
                        worker_name: worker_name.map(Box::new),
                    })
                }
            }

            _ => Ok(InstanceCreationType::WitWorker {
                component_info: None,
                worker_name: worker_name.map(Box::new),
            }),
        }
    }

    pub fn from_raw(
        component_and_exports: Vec<(ComponentDependencyKey, &Vec<AnalysedExport>)>,
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
