use std::fmt::{self, Display, Formatter};

use golem_service_base::model::TemplateMetadata;
use golem_wasm_ast::{
    analysis::{AnalysedExport, AnalysedFunction, AnalysisContext, AnalysisFailure},
    component::Component,
    IgnoreAllButMetadata,
};

pub fn process_template(data: &[u8]) -> Result<TemplateMetadata, TemplateProcessingError> {
    let component = Component::<IgnoreAllButMetadata>::from_bytes(data)
        .map_err(|e| TemplateProcessingError::Parsing(e))?;

    let producers = component
        .get_all_producers()
        .into_iter()
        .map(|producers| producers.into())
        .collect::<Vec<_>>();

    let state = AnalysisContext::new(component);

    let mut exports = state
        .get_top_level_exports()
        .map_err(TemplateProcessingError::Analysis)?;

    add_resource_drops(&mut exports);

    let exports = exports
        .into_iter()
        .map(|export| export.into())
        .collect::<Vec<_>>();

    Ok(TemplateMetadata { exports, producers })
}

#[derive(Debug, thiserror::Error)]
pub enum TemplateProcessingError {
    Parsing(String),
    Analysis(AnalysisFailure),
}

impl Display for TemplateProcessingError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            TemplateProcessingError::Parsing(e) => write!(f, "Parsing error: {}", e),
            TemplateProcessingError::Analysis(source) => {
                let error = match source {
                    AnalysisFailure::Failed(error) => error,
                };
                write!(f, "Analysis error: {}", error)
            }
        }
    }
}

fn add_resource_drops(exports: &mut Vec<AnalysedExport>) {
    // Components are not exporting explicit drop functions for exported resources, but
    // worker executor does. So we keep golem-wasm-ast as an universal library and extend
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
