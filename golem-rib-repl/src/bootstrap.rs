use rustyline::Editor;
use crate::dependency_manager::ComponentDependency;
use crate::history::RibReplHistory;
use crate::rib_edit::RibEdit;
use crate::rib_repl::RibRepl;

pub async fn bootstrap(rib_repl: &RibRepl, editor: &mut Editor<RibEdit, RibReplHistory>) -> Result<ComponentDependency, ReplBootstrapError> {
    if rib_repl.history_file_path.exists() {
        if let Err(err) = editor.load_history(&rib_repl.history_file_path) {
            return Err(ReplBootstrapError::ReplHistoryFileError(
                format!("Failed to load history: {}. Starting with an empty history.", err),
            ))
        }
    }

    match rib_repl.component_details {
        Some(ref details) => {
            rib_repl.dependency_manager
                .add_component_dependency(&details.source_path, details.component_name.clone())
                .await.map_err(ReplBootstrapError::ComponentLoadError)
        }
        None => {
            let dependencies =
                rib_repl.dependency_manager.add_components()
                    .await;

            match dependencies {
                Ok(dependencies) => {
                    let component_dependencies = dependencies.component_dependencies;

                    match &component_dependencies.len() {
                        0 => Err(ReplBootstrapError::NoComponentsFound),
                        1 => Ok(component_dependencies[0].clone()),
                        _ => Err(ReplBootstrapError::MultipleComponentsFound(
                            "multiple components detected. Rib Repl currently support only a single component".to_string(),
                        )),
                    }
                }
                Err(err) => {
                    Err(ReplBootstrapError::ComponentLoadError(
                        format!("Failed to register components: {}", err),
                    ))
                }
            }
        }
    }
}

pub enum ReplBootstrapError {
    // Currently not supported
    // So either the context should have only 1 component
    // or specifically specify the component when starting the REPL
    // In future, when Rib supports multiple components (which may require the need
    // of root package names being the component name)
    MultipleComponentsFound(String),
    NoComponentsFound,
    ComponentLoadError(String),
    Internal(String),
    ReplHistoryFileError(String),
}
