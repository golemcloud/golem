// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::quota::QuotaError;
use crate::sharding::error::ShardManagerError;
use golem_api_grpc::proto::golem;
use golem_api_grpc::proto::golem::shardmanager::v1::shard_manager_error;
use golem_common::base_model::api;
use golem_common::metrics::api::ApiErrorDetails;
use std::fmt::Debug;
use std::fmt::Formatter;

impl From<ShardManagerError> for golem::shardmanager::v1::ShardManagerError {
    fn from(value: ShardManagerError) -> golem::shardmanager::v1::ShardManagerError {
        let error = |cons: fn(golem::common::ErrorBody) -> shard_manager_error::Error,
                     error: String,
                     code: &str| {
            golem::shardmanager::v1::ShardManagerError {
                error: Some(cons(golem::common::ErrorBody {
                    error,
                    code: code.to_string(),
                })),
            }
        };

        match value {
            ShardManagerError::NoSourceIpForPod => error(
                shard_manager_error::Error::InvalidRequest,
                "NoSourceIpForPod".to_string(),
                api::error_code::VALIDATION_ERROR,
            ),
            ShardManagerError::FailedAddressResolveForPod => error(
                shard_manager_error::Error::Unknown,
                "FailedAddressResolveForPod".to_string(),
                api::error_code::INTERNAL_ROUTING_FAILURE,
            ),
            ShardManagerError::Timeout => error(
                shard_manager_error::Error::Timeout,
                "Timeout".to_string(),
                api::error_code::INTERNAL_ROUTING_FAILURE,
            ),
            ShardManagerError::GrpcError(status) => error(
                shard_manager_error::Error::Unknown,
                status.to_string(),
                api::error_code::INTERNAL_DEPENDENCY_FAILURE,
            ),
            ShardManagerError::NoResult => error(
                shard_manager_error::Error::Unknown,
                "NoResult".to_string(),
                api::error_code::INTERNAL_UNKNOWN,
            ),
            ShardManagerError::WorkerExecutionError(details) => error(
                shard_manager_error::Error::Unknown,
                details.to_string(),
                api::error_code::INTERNAL_AGENT_EXECUTION_FAILED,
            ),
            ShardManagerError::SerializationError(details) => error(
                shard_manager_error::Error::Unknown,
                details,
                api::error_code::INTERNAL_UNKNOWN,
            ),
            ShardManagerError::RepoError(err) => error(
                shard_manager_error::Error::Unknown,
                err.to_string(),
                api::error_code::INTERNAL_DEPENDENCY_FAILURE,
            ),
            ShardManagerError::MigrationError(err) => error(
                shard_manager_error::Error::Unknown,
                err.to_string(),
                api::error_code::INTERNAL_DEPENDENCY_FAILURE,
            ),
            ShardManagerError::IoError(err) => error(
                shard_manager_error::Error::Unknown,
                err.to_string(),
                api::error_code::INTERNAL_FILESYSTEM_ERROR,
            ),
        }
    }
}

impl From<QuotaError> for golem::shardmanager::v1::QuotaError {
    fn from(value: QuotaError) -> golem::shardmanager::v1::QuotaError {
        use golem::shardmanager::v1::quota_error as grpc_quota_error;
        match value {
            QuotaError::LeaseNotFound {
                resource_definition_id,
            } => golem::shardmanager::v1::QuotaError {
                error: Some(grpc_quota_error::Error::LeaseNotFound(
                    golem::common::ErrorBody {
                        error: format!("Did not find lease for {resource_definition_id}"),
                        code: api::error_code::RESOURCE_NOT_FOUND.to_string(),
                    },
                )),
            },
            QuotaError::StaleEpoch {
                resource_definition_id,
                current,
                provided,
            } => golem::shardmanager::v1::QuotaError {
                error: Some(grpc_quota_error::Error::StaleEpoch(
                    golem::common::ErrorBody {
                        error: format!(
                            "Stale epoch provided for {resource_definition_id} (provided: {provided}, current: {current}) "
                        ),
                        code: api::error_code::CONCURRENT_UPDATE.to_string(),
                    },
                )),
            },
            QuotaError::InternalError(_) => golem::shardmanager::v1::QuotaError {
                error: Some(grpc_quota_error::Error::Internal(
                    golem::common::ErrorBody {
                        error: value.to_string(),
                        code: api::error_code::INTERNAL_UNKNOWN.to_string(),
                    },
                )),
            },
        }
    }
}

pub struct ShardManagerTraceErrorKind<'a>(pub &'a golem::shardmanager::v1::ShardManagerError);

impl Debug for ShardManagerTraceErrorKind<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl ApiErrorDetails for ShardManagerTraceErrorKind<'_> {
    fn trace_error_kind(&self) -> &'static str {
        match &self.0.error {
            None => "None",
            Some(error) => match error {
                shard_manager_error::Error::InvalidRequest(_) => "InvalidRequest",
                shard_manager_error::Error::Timeout(_) => "Timeout",
                shard_manager_error::Error::Unknown(_) => "Unknown",
            },
        }
    }

    fn is_expected(&self) -> bool {
        match &self.0.error {
            None => false,
            Some(error) => match error {
                shard_manager_error::Error::InvalidRequest(_) => true,
                shard_manager_error::Error::Timeout(_) => true,
                shard_manager_error::Error::Unknown(_) => false,
            },
        }
    }

    fn take_cause(&mut self) -> Option<anyhow::Error> {
        None
    }
}
