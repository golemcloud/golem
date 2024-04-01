use std::fmt::Debug;
use std::fmt::Display;
use std::str::FromStr;

use bincode::{Decode, Encode};
use poem_openapi::{Enum, NewType};
use serde::{Deserialize, Serialize, Serializer};
use Iterator;

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, Encode, Decode, NewType)]
pub struct ApiDefinitionId(pub String);

impl Display for ApiDefinitionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, Encode, Decode, NewType)]
pub struct Version(pub String);

impl Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub trait HasApiDefinitionId {
    fn get_api_definition_id(&self) -> ApiDefinitionId;
}

pub trait HasVersion {
    fn get_version(&self) -> Version;
}
