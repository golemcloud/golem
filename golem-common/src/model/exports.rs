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

use golem_wasm_ast::analysis::{AnalysedExport, AnalysedFunction};

use rib::{ParsedFunctionName, ParsedFunctionReference, ParsedFunctionSite};

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
