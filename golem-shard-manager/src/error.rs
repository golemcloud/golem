use std::error::Error;
use std::fmt::{Display, Formatter};

use golem_api_grpc::proto::golem;
use tonic::Status;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ShardManagerError {
    InvalidRequest { details: String },
    Timeout { details: String },
    Unknown(String),
}

impl ShardManagerError {
    pub fn invalid_request(details: impl Into<String>) -> Self {
        ShardManagerError::InvalidRequest {
            details: details.into(),
        }
    }

    pub fn timeout(details: impl Into<String>) -> Self {
        ShardManagerError::Timeout {
            details: details.into(),
        }
    }

    pub fn unknown(details: impl Into<String>) -> Self {
        ShardManagerError::Unknown(details.into())
    }
}

impl Display for ShardManagerError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ShardManagerError::InvalidRequest { details } => {
                write!(f, "Invalid request: {}", details)
            }
            ShardManagerError::Timeout { details } => write!(f, "Timeout: {}", details),
            ShardManagerError::Unknown(s) => write!(f, "Unknown error: {}", s),
        }
    }
}

impl From<anyhow::Error> for ShardManagerError {
    fn from(value: anyhow::Error) -> Self {
        // TODO: downcast to specific errors
        ShardManagerError::Unknown(format!("{value}"))
    }
}

impl From<ShardManagerError> for tonic::Status {
    fn from(value: ShardManagerError) -> Self {
        Status::internal(format!("{value}"))
    }
}

impl From<ShardManagerError> for golem::common::ShardManagerError {
    fn from(value: ShardManagerError) -> golem::common::ShardManagerError {
        match value {
            ShardManagerError::InvalidRequest { details } => golem::common::ShardManagerError {
                error: Some(golem::common::shard_manager_error::Error::InvalidRequest(
                    golem::common::ErrorBody { error: details },
                )),
            },
            ShardManagerError::Timeout { details } => golem::common::ShardManagerError {
                error: Some(golem::common::shard_manager_error::Error::Timeout(
                    golem::common::ErrorBody { error: details },
                )),
            },
            ShardManagerError::Unknown(s) => golem::common::ShardManagerError {
                error: Some(golem::common::shard_manager_error::Error::Unknown(
                    golem::common::ErrorBody { error: s },
                )),
            },
        }
    }
}

impl Error for ShardManagerError {
    fn description(&self) -> &str {
        match self {
            ShardManagerError::InvalidRequest { .. } => "Invalid request",
            ShardManagerError::Timeout { .. } => "Timeout",
            ShardManagerError::Unknown { .. } => "Unknown error",
        }
    }
}
