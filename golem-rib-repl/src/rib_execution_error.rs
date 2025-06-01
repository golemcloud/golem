use rib::{RibCompilationError, RibRuntimeError};
use std::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum RibExecutionError {
    RibCompilationError(RibCompilationError),
    RibRuntimeError(RibRuntimeError),
    Custom(String),
}

impl std::error::Error for RibExecutionError {}

impl Display for RibExecutionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RibExecutionError::RibCompilationError(err) => write!(f, "{}", err),
            RibExecutionError::RibRuntimeError(err) => write!(f, "{}", err),
            RibExecutionError::Custom(msg) => write!(f, "{}", msg),
        }
    }
}
