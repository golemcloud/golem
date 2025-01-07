// Copyright 2024-2025 Golem Cloud
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

use golem_wasm_ast::analysis::{AnalysedExport, AnalysedFunction, AnalysedInstance};

use rib::{ParsedFunctionName, ParsedFunctionReference, ParsedFunctionSite};

pub trait AnalysedExportExtensions {
    fn function_names(&self) -> Vec<String>;
}

impl AnalysedExportExtensions for AnalysedExport {
    fn function_names(&self) -> Vec<String> {
        match self {
            AnalysedExport::Instance(instance) => instance
                .functions
                .iter()
                .map(|function| format!("{}.{{{}}}", instance.name, function.name))
                .collect(),
            AnalysedExport::Function(function) => vec![function.name.clone()],
        }
    }
}

pub fn instances(exports: &Vec<AnalysedExport>) -> Vec<AnalysedInstance> {
    let mut instances = vec![];
    for export in exports {
        if let AnalysedExport::Instance(instance) = export {
            instances.push(instance.clone())
        }
    }
    instances
}

pub fn functions(exports: &Vec<AnalysedExport>) -> Vec<AnalysedFunction> {
    let mut functions = vec![];
    for export in exports {
        if let AnalysedExport::Function(function) = export {
            functions.push(function.clone())
        }
    }
    functions
}

pub fn function_by_name(
    exports: &Vec<AnalysedExport>,
    name: &str,
) -> Result<Option<AnalysedFunction>, String> {
    let parsed = ParsedFunctionName::parse(name)?;

    match &parsed.site().interface_name() {
        None => Ok(functions(exports).iter().find(|f| f.name == *name).cloned()),
        Some(interface_name) => {
            let exported_function = instances(exports)
                .iter()
                .find(|instance| instance.name == *interface_name)
                .and_then(|instance| {
                    instance
                        .functions
                        .iter()
                        .find(|f| f.name == parsed.function().function_name())
                        .cloned()
                });
            if exported_function.is_none() {
                match parsed.method_as_static() {
                    Some(parsed_static) => Ok(instances(exports)
                        .iter()
                        .find(|instance| instance.name == *interface_name)
                        .and_then(|instance| {
                            instance
                                .functions
                                .iter()
                                .find(|f| f.name == parsed_static.function().function_name())
                                .cloned()
                        })),
                    None => Ok(None),
                }
            } else {
                Ok(exported_function)
            }
        }
    }
}

pub fn find_resource_site(
    exports: &[AnalysedExport],
    resource_name: &str,
) -> Option<ParsedFunctionSite> {
    fn find_resource_site_impl(
        site: ParsedFunctionSite,
        functions: &[AnalysedFunction],
        resource_name: &str,
    ) -> Option<ParsedFunctionSite> {
        let constructor = ParsedFunctionName::new(
            site.clone(),
            ParsedFunctionReference::RawResourceConstructor {
                resource: resource_name.to_string(),
            },
        );
        if functions
            .iter()
            .any(|f| f.name == constructor.function().function_name())
        {
            Some(site)
        } else {
            None
        }
    }

    let global_functions = exports
        .iter()
        .filter_map(|export| {
            if let AnalysedExport::Function(f) = export {
                Some(f.clone())
            } else {
                None
            }
        })
        .collect::<Vec<AnalysedFunction>>();

    if let Some(result) =
        find_resource_site_impl(ParsedFunctionSite::Global, &global_functions, resource_name)
    {
        Some(result)
    } else {
        for export in exports {
            if let AnalysedExport::Instance(instance) = export {
                if let Some(result) = find_resource_site_impl(
                    ParsedFunctionSite::Interface {
                        name: instance.name.clone(),
                    },
                    &instance.functions,
                    resource_name,
                ) {
                    return Some(result);
                }
            }
        }

        None
    }
}
