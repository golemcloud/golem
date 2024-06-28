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

use std::fmt::{self, Display, Formatter};

use golem_wasm_ast::{
    analysis::{AnalysedExport, AnalysedFunction, AnalysisContext, AnalysisFailure},
    component::Component,
    IgnoreAllButMetadata,
};

use golem_service_base::model::ComponentMetadata;

pub fn process_component(data: &[u8]) -> Result<ComponentMetadata, ComponentProcessingError> {
    let component = Component::<IgnoreAllButMetadata>::from_bytes(data)
        .map_err(ComponentProcessingError::Parsing)?;

    let producers = component
        .get_all_producers()
        .into_iter()
        .map(|producers| producers.into())
        .collect::<Vec<_>>();

    let state = AnalysisContext::new(component);

    let mut exports = state
        .get_top_level_exports()
        .map_err(ComponentProcessingError::Analysis)?;

    add_resource_drops(&mut exports);

    let exports = exports
        .into_iter()
        .map(|export| export.into())
        .collect::<Vec<_>>();

    let memories = state
        .get_all_memories()
        .map_err(ComponentProcessingError::Analysis)?
        .into_iter()
        .map(|mem| mem.into())
        .collect();

    Ok(ComponentMetadata {
        exports,
        producers,
        memories,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum ComponentProcessingError {
    Parsing(String),
    Analysis(AnalysisFailure),
}

impl Display for ComponentProcessingError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ComponentProcessingError::Parsing(e) => write!(f, "Parsing error: {}", e),
            ComponentProcessingError::Analysis(source) => {
                let AnalysisFailure::Failed(error) = source;
                write!(f, "Analysis error: {}", error)
            }
        }
    }
}

fn add_resource_drops(exports: &mut Vec<AnalysedExport>) {
    // Components are not exporting explicit drop functions for exported resources, but
    // worker executor does. So we keep golem-wasm-ast as a universal library and extend
    // its result with the explicit drops here, for each resource, identified by an exported
    // constructor.

    let mut to_add = Vec::new();
    for export in exports.iter_mut() {
        match export {
            AnalysedExport::Function(fun) => {
                if fun.is_constructor() {
                    let drop_name = fun.name.replace("[constructor]", "[drop]");
                    to_add.push(AnalysedExport::Function(AnalysedFunction {
                        name: drop_name,
                        ..fun.clone()
                    }));
                }
            }
            AnalysedExport::Instance(instance) => {
                let mut to_add = Vec::new();
                for fun in &instance.funcs {
                    if fun.is_constructor() {
                        let drop_name = fun.name.replace("[constructor]", "[drop]");
                        to_add.push(AnalysedFunction {
                            name: drop_name,
                            ..fun.clone()
                        });
                    }
                }
                instance.funcs.extend(to_add.into_iter());
            }
        }
    }

    exports.extend(to_add);
}
