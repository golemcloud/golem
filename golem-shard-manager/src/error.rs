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

impl From<ShardManagerError> for golem::shardmanager::ShardManagerError {
    fn from(value: ShardManagerError) -> golem::shardmanager::ShardManagerError {
        match value {
            ShardManagerError::InvalidRequest { details } => {
                golem::shardmanager::ShardManagerError {
                    error: Some(
                        golem::shardmanager::shard_manager_error::Error::InvalidRequest(
                            golem::common::ErrorBody { error: details },
                        ),
                    ),
                }
            }
            ShardManagerError::Timeout { details } => golem::shardmanager::ShardManagerError {
                error: Some(golem::shardmanager::shard_manager_error::Error::Timeout(
                    golem::common::ErrorBody { error: details },
                )),
            },
            ShardManagerError::Unknown(s) => golem::shardmanager::ShardManagerError {
                error: Some(golem::shardmanager::shard_manager_error::Error::Unknown(
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
