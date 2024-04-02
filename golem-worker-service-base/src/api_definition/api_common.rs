use std::fmt::Debug;
use std::fmt::Display;

use bincode::{Decode, Encode};
use poem_openapi::NewType;
use serde::{Deserialize, Serialize};

use crate::worker_binding::GolemWorkerBinding;

// Common to API definitions regardless of different protocols
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, Encode, Decode, NewType)]
pub struct ApiDefinitionId(pub String);

impl Display for ApiDefinitionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, Encode, Decode, NewType)]
pub struct ApiVersion(pub String);

impl Display for ApiVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// Constraints applicable to any type of API Definition
pub trait HasApiDefinitionId {
    fn get_api_definition_id(&self) -> ApiDefinitionId;
}

pub trait HasVersion {
    fn get_version(&self) -> ApiVersion;
}

pub trait HasGolemWorkerBindings {
    fn get_golem_worker_bindings(&self) -> Vec<GolemWorkerBinding>;
}
