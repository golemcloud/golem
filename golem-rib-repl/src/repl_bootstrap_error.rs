use std::fmt::{Display, Formatter};

/// Represents errors that can occur during the bootstrap phase of the Rib REPL environment.
#[derive(Debug, Clone, PartialEq)]
pub enum ReplBootstrapError {
    /// Multiple components were found, but the REPL requires a single component context.
    ///
    /// To resolve this, either:
    /// - Ensure the context includes only one component, or
    /// - Explicitly specify the component to load when starting the REPL.
    ///
    /// In the future, Rib will support multiple components
    MultipleComponentsFound(String),

    /// No components were found in the given context.
    NoComponentsFound,

    /// Failed to load a specified component.
    ComponentLoadError(String),

    /// Failed to read from or write to the REPL history file.
    ReplHistoryFileError(String),
}

impl std::error::Error for ReplBootstrapError {}

impl Display for ReplBootstrapError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplBootstrapError::MultipleComponentsFound(msg) => {
                write!(f, "Multiple components found: {}", msg)
            }
            ReplBootstrapError::NoComponentsFound => {
                write!(f, "No components found in the given context")
            }
            ReplBootstrapError::ComponentLoadError(msg) => {
                write!(f, "Failed to load component: {}", msg)
            }
            ReplBootstrapError::ReplHistoryFileError(msg) => {
                write!(f, "Failed to read/write REPL history file: {}", msg)
            }
        }
    }
}
