pub mod http;

use std::fmt::Debug;
use std::fmt::Display;

use bincode::{Decode, Encode};
use poem_openapi::NewType;
use serde::{Deserialize, Serialize};

use crate::gateway_binding::WorkerBinding;

// Common to API definitions regardless of different protocols
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, Encode, Decode, NewType)]
pub struct ApiDefinitionId(pub String);

impl From<String> for ApiDefinitionId {
    fn from(id: String) -> Self {
        ApiDefinitionId(id)
    }
}

impl Display for ApiDefinitionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, Encode, Decode, NewType)]
pub struct ApiVersion(pub String);

impl From<String> for ApiVersion {
    fn from(id: String) -> Self {
        ApiVersion(id)
    }
}

impl Display for ApiVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub trait HasGolemWorkerBindings {
    fn get_golem_worker_bindings(&self) -> Vec<WorkerBinding>;
}