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

        // Taken before the match moves `value`: the stale-epoch arm reports the variant's own
        // message, which names the shard, the claimant and both epochs.
        let value_message = value.to_string();

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
            ShardManagerError::ConcurrentModification => error(
                shard_manager_error::Error::Unknown,
                "Concurrent modification of the persisted shard state".to_string(),
                api::error_code::CONCURRENT_UPDATE,
            ),
            // Both lease refusals are client errors, not server faults: `InvalidRequest` is what
            // `ShardManagerTraceErrorKind::is_expected` treats as expected, so a stale claim does
            // not read as an incident. The codes match the quota counterparts below.
            ShardManagerError::ShardLeaseNotFound { executor_id } => error(
                shard_manager_error::Error::InvalidRequest,
                format!("Did not find a shard lease for executor {executor_id}"),
                api::error_code::RESOURCE_NOT_FOUND,
            ),
            ShardManagerError::StaleShardEpoch { .. } => error(
                shard_manager_error::Error::InvalidRequest,
                value_message.clone(),
                api::error_code::CONCURRENT_UPDATE,
            ),
            ShardManagerError::LeadershipLost { .. } => error(
                shard_manager_error::Error::Unknown,
                "Leadership of the shard manager was lost".to_string(),
                api::error_code::INTERNAL_SHARDING_NOT_READY,
            ),
            ShardManagerError::LeaseLostWhileCampaigning(lost) => error(
                shard_manager_error::Error::Unknown,
                lost.to_string(),
                api::error_code::INTERNAL_SHARDING_NOT_READY,
            ),
            ShardManagerError::ShutdownRequested => error(
                shard_manager_error::Error::Unknown,
                "The shard manager is shutting down".to_string(),
                api::error_code::INTERNAL_SHARDING_NOT_READY,
            ),
            ShardManagerError::RepoError(err) => error(
                shard_manager_error::Error::Unknown,
                err.to_string(),
                api::error_code::INTERNAL_DEPENDENCY_FAILURE,
            ),
            ShardManagerError::EtcdError(err) => error(
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
            ShardManagerError::Internal(details) => error(
                shard_manager_error::Error::Unknown,
                details,
                api::error_code::INTERNAL_UNKNOWN,
            ),
        }
    }
}

/// The failure of a shard lease operation, as `RenewShardLease` and `Deregister` report it.
///
/// The **arm** carries the semantics and is what the executor branches on: `lease_not_found` means
/// re-register with a fresh id, `stale_epoch` means keep the current set and retry, and `internal`
/// is transport-class - keep the lease and try again. The body is taken from the
/// [`golem::shardmanager::v1::ShardManagerError`] mapping above so no code string is written twice;
/// that is also what keeps `CONCURRENT_UPDATE` on the `internal` arm of a lost compare-and-swap,
/// where `QuotaError` would have flattened it to `INTERNAL_UNKNOWN`.
impl From<ShardManagerError> for golem::shardmanager::v1::ShardLeaseError {
    fn from(value: ShardManagerError) -> golem::shardmanager::v1::ShardLeaseError {
        use golem::shardmanager::v1::shard_lease_error as grpc_shard_lease_error;

        let arm: fn(golem::common::ErrorBody) -> grpc_shard_lease_error::Error = match &value {
            ShardManagerError::ShardLeaseNotFound { .. } => {
                grpc_shard_lease_error::Error::LeaseNotFound
            }
            ShardManagerError::StaleShardEpoch { .. } => grpc_shard_lease_error::Error::StaleEpoch,
            _ => grpc_shard_lease_error::Error::Internal,
        };

        let body = match golem::shardmanager::v1::ShardManagerError::from(value).error {
            Some(shard_manager_error::Error::InvalidRequest(body))
            | Some(shard_manager_error::Error::Timeout(body))
            | Some(shard_manager_error::Error::Unknown(body)) => body,
            None => golem::common::ErrorBody {
                error: "unknown shard lease error".to_string(),
                code: api::error_code::INTERNAL_UNKNOWN.to_string(),
            },
        };

        golem::shardmanager::v1::ShardLeaseError {
            error: Some(arm(body)),
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

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;
    use crate::sharding::error::ShardManagerError;
    use crate::sharding::{ExecutorId, ShardEpoch};
    use golem_common::model::ShardId;
    use uuid::Uuid;

    fn arms(error: ShardManagerError) -> golem::shardmanager::v1::shard_lease_error::Error {
        golem::shardmanager::v1::ShardLeaseError::from(error)
            .error
            .expect("every ShardManagerError must map to an arm")
    }

    fn body(arm: &golem::shardmanager::v1::shard_lease_error::Error) -> golem::common::ErrorBody {
        use golem::shardmanager::v1::shard_lease_error::Error;
        match arm {
            Error::LeaseNotFound(body) | Error::StaleEpoch(body) | Error::Internal(body) => {
                body.clone()
            }
        }
    }

    /// The cross-track contract: this mapping is what the executor branches on
    /// (`lease_not_found` => re-register under a fresh id; `stale_epoch` =>
    /// keep the set and retry; `internal` => treat as transport). A refusal
    /// that landed on the wrong arm would silently change the executor's
    /// reaction to it.
    #[test]
    fn each_lease_refusal_maps_to_the_arm_the_executor_branches_on() {
        use golem::shardmanager::v1::shard_lease_error::Error;

        let not_found = arms(ShardManagerError::ShardLeaseNotFound {
            executor_id: ExecutorId(Uuid::nil()),
        });
        assert!(matches!(not_found, Error::LeaseNotFound(_)));
        assert_eq!(body(&not_found).code, api::error_code::RESOURCE_NOT_FOUND);

        let stale = arms(ShardManagerError::StaleShardEpoch {
            executor_id: ExecutorId(Uuid::nil()),
            shard_id: ShardId::new(3),
            expected: Some(ShardEpoch(7)),
            provided: ShardEpoch(6),
        });
        assert!(matches!(stale, Error::StaleEpoch(_)));
        assert_eq!(body(&stale).code, api::error_code::CONCURRENT_UPDATE);
    }

    /// A lost compare-and-swap is `internal` on the wire, but it must keep the
    /// `CONCURRENT_UPDATE` code rather than flattening to `INTERNAL_UNKNOWN`
    /// the way `QuotaError` does: it is a retry-the-write condition, not an
    /// unknown fault.
    #[test]
    fn a_lost_compare_and_swap_is_internal_but_keeps_its_concurrent_update_code() {
        use golem::shardmanager::v1::shard_lease_error::Error;

        let arm = arms(ShardManagerError::ConcurrentModification);

        assert!(matches!(arm, Error::Internal(_)));
        assert_eq!(body(&arm).code, api::error_code::CONCURRENT_UPDATE);
    }
}
