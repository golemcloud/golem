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

//! The inbound request metrics of the shard manager's gRPC service.
//!
//! Every RPC that opens a request recorder must complete it with the result it returns. A recorder
//! dropped without a result counts as `api_failure_seconds{error="Drop"}`, so an RPC that forgets to
//! complete it reports each success as a failure. These tests call the real service and read the
//! process-wide default registry that the shard manager's `/metrics` endpoint exports. Each RPC
//! named here is called only by these tests in this binary, so the deltas they read are their own.

use crate::shard_management::{
    TestWorkerExecutors, balanced_pair, claim_of, executor, new_shard_management,
};
use async_trait::async_trait;
use golem_api_grpc::proto::golem::shardmanager;
use golem_api_grpc::proto::golem::shardmanager::v1::shard_manager_service_server::ShardManagerService;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::quota::{ResourceDefinition, ResourceDefinitionId, ResourceName};
use golem_service_base::repo::Blob;
use golem_shard_manager::config::QuotaServiceConfig;
use golem_shard_manager::quota::quota_repo::{
    QuotaLeaseRecord, QuotaRepoError, QuotaResourceRecord, StoredQuotaResource, StoredQuotaState,
};
use golem_shard_manager::quota::resource_definition_fetcher::FetchError;
use golem_shard_manager::quota::{QuotaRepo, QuotaService, ResourceDefinitionFetcher};
use golem_shard_manager::{
    ExecutorId, ExternalRevision, ShardEpoch, ShardManagerError, ShardManagerServiceImpl,
};
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::Arc;
use test_r::test;

/// A quota store that no RPC under test reaches.
struct UnusedQuotaRepo;

#[async_trait]
impl QuotaRepo for UnusedQuotaRepo {
    async fn save_lease_change(
        &self,
        _resource: &QuotaResourceRecord,
        _previous_revision: ExternalRevision,
        _lease: &QuotaLeaseRecord,
        _expired_pods: &[(Blob<IpAddr>, i32)],
    ) -> Result<ExternalRevision, QuotaRepoError> {
        unreachable!("the shard lease RPCs do not write quota state")
    }

    async fn save_lease_release(
        &self,
        _resource: &QuotaResourceRecord,
        _previous_revision: ExternalRevision,
        _pod_ip: Blob<IpAddr>,
        _pod_port: i32,
    ) -> Result<ExternalRevision, QuotaRepoError> {
        unreachable!("the shard lease RPCs do not write quota state")
    }

    async fn save_resource(
        &self,
        _record: &QuotaResourceRecord,
        _previous_revision: ExternalRevision,
    ) -> Result<ExternalRevision, QuotaRepoError> {
        unreachable!("the shard lease RPCs do not write quota state")
    }

    async fn delete_resource_and_leases(
        &self,
        _resource_definition_id: ResourceDefinitionId,
    ) -> Result<(), QuotaRepoError> {
        unreachable!("the shard lease RPCs do not write quota state")
    }

    async fn get_all(&self) -> Result<StoredQuotaState, QuotaRepoError> {
        unreachable!("the shard lease RPCs do not read quota state")
    }

    async fn get_resource(
        &self,
        _resource_definition_id: ResourceDefinitionId,
    ) -> Result<Option<(StoredQuotaResource, Vec<QuotaLeaseRecord>)>, QuotaRepoError> {
        unreachable!("the shard lease RPCs do not read quota state")
    }

    async fn delete_leases_for_resource(
        &self,
        _resource_definition_id: ResourceDefinitionId,
    ) -> Result<(), QuotaRepoError> {
        unreachable!("the shard lease RPCs do not write quota state")
    }
}

/// A resource definition source that no RPC under test reaches.
struct UnusedFetcher;

#[async_trait]
impl ResourceDefinitionFetcher for UnusedFetcher {
    async fn fetch_by_id(
        &self,
        _id: ResourceDefinitionId,
    ) -> Result<ResourceDefinition, FetchError> {
        unreachable!("the shard lease RPCs do not fetch resource definitions")
    }

    async fn resolve_by_name(
        &self,
        _environment_id: EnvironmentId,
        _name: ResourceName,
    ) -> Result<ResourceDefinition, FetchError> {
        unreachable!("the shard lease RPCs do not fetch resource definitions")
    }

    async fn invalidate(&self, _environment_id: EnvironmentId, _name: ResourceName) {}

    async fn invalidate_all(&self) {}
}

fn quota_service() -> Arc<QuotaService> {
    QuotaService::new(
        QuotaServiceConfig::default(),
        Arc::new(UnusedFetcher),
        Arc::new(UnusedQuotaRepo),
    )
}

/// The number of observations in the histogram `family` whose labels include every pair in
/// `labels`, or zero while no such series exists.
fn histogram_count(family: &str, labels: &[(&str, &str)]) -> u64 {
    prometheus::default_registry()
        .gather()
        .into_iter()
        .filter(|f| f.name() == family)
        .flat_map(|f| f.get_metric().to_vec())
        .filter(|metric| {
            labels.iter().all(|(name, value)| {
                metric
                    .get_label()
                    .iter()
                    .any(|label| label.name() == *name && label.value() == *value)
            })
        })
        .map(|metric| metric.get_histogram().get_sample_count())
        .sum()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ApiCounts {
    success: u64,
    failure: u64,
    dropped: u64,
}

fn api_counts(api: &str) -> ApiCounts {
    let api_labels = [("api", api), ("api_type", "grpc")];
    ApiCounts {
        success: histogram_count("api_success_seconds", &api_labels),
        failure: histogram_count("api_failure_seconds", &api_labels),
        dropped: histogram_count(
            "api_failure_seconds",
            &[("api", api), ("api_type", "grpc"), ("error", "Drop")],
        ),
    }
}

fn failure_count(api: &str, error: &str) -> u64 {
    histogram_count(
        "api_failure_seconds",
        &[("api", api), ("api_type", "grpc"), ("error", error)],
    )
}

fn epoch_entries(
    claim: &BTreeMap<golem_common::model::ShardId, ShardEpoch>,
) -> Vec<shardmanager::ShardEpochEntry> {
    claim
        .iter()
        .map(|(shard_id, epoch)| shardmanager::ShardEpochEntry {
            shard_id: Some((*shard_id).into()),
            epoch: epoch.0,
        })
        .collect()
}

fn renew_request(
    executor_id: ExecutorId,
    claim: &BTreeMap<golem_common::model::ShardId, ShardEpoch>,
) -> tonic::Request<shardmanager::v1::RenewShardLeaseRequest> {
    tonic::Request::new(shardmanager::v1::RenewShardLeaseRequest {
        executor_id: executor_id.0.to_string(),
        shard_epochs: epoch_entries(claim),
    })
}

fn deregister_request(
    executor_id: ExecutorId,
    claim: &BTreeMap<golem_common::model::ShardId, ShardEpoch>,
) -> tonic::Request<shardmanager::v1::DeregisterRequest> {
    tonic::Request::new(shardmanager::v1::DeregisterRequest {
        executor_id: executor_id.0.to_string(),
        shard_epochs: epoch_entries(claim),
    })
}

#[test]
async fn a_served_routing_table_counts_as_a_success() {
    let (shard_management, _persistence, mut join_set) =
        new_shard_management(balanced_pair(), Arc::new(TestWorkerExecutors::default())).await;
    let service = ShardManagerServiceImpl::new(Arc::new(shard_management), quota_service());

    let before = api_counts("get_routing_table");
    let response = service
        .get_routing_table(tonic::Request::new(
            shardmanager::v1::GetRoutingTableRequest {},
        ))
        .await
        .expect("the routing table is always served")
        .into_inner();
    assert!(matches!(
        response.result,
        Some(shardmanager::v1::get_routing_table_response::Result::Success(_))
    ));
    let after = api_counts("get_routing_table");

    assert_eq!(
        after,
        ApiCounts {
            success: before.success + 1,
            ..before
        },
        "a served routing table must count once as a success and never as a dropped failure"
    );

    join_set.abort_all();
}

#[test]
async fn shard_lease_renewals_count_their_real_outcome() {
    let (shard_management, persistence, mut join_set) =
        new_shard_management(balanced_pair(), Arc::new(TestWorkerExecutors::default())).await;
    let service = ShardManagerServiceImpl::new(Arc::new(shard_management), quota_service());
    let claim = claim_of(&persistence.latest().await, executor(1));

    let before = api_counts("renew_shard_lease");
    let not_found_before = failure_count("renew_shard_lease", "LeaseNotFound");

    let granted = service
        .renew_shard_lease(renew_request(executor(1), &claim))
        .await
        .expect("a well-formed renewal is answered")
        .into_inner();
    assert!(matches!(
        granted.result,
        Some(shardmanager::v1::renew_shard_lease_response::Result::Success(_))
    ));

    let refused = service
        .renew_shard_lease(renew_request(executor(9), &BTreeMap::new()))
        .await
        .expect("a refused renewal is answered, not a transport error")
        .into_inner();
    assert!(matches!(
        refused.result,
        Some(shardmanager::v1::renew_shard_lease_response::Result::Failure(_))
    ));

    let after = api_counts("renew_shard_lease");
    assert_eq!(
        after,
        ApiCounts {
            success: before.success + 1,
            failure: before.failure + 1,
            dropped: before.dropped,
        }
    );
    assert_eq!(
        failure_count("renew_shard_lease", "LeaseNotFound"),
        not_found_before + 1,
        "an executor the manager does not know is refused as LeaseNotFound"
    );

    join_set.abort_all();
}

#[test]
async fn deregistrations_count_their_real_outcome() {
    let (shard_management, persistence, mut join_set) =
        new_shard_management(balanced_pair(), Arc::new(TestWorkerExecutors::default())).await;
    let service = ShardManagerServiceImpl::new(Arc::new(shard_management), quota_service());
    let state = persistence.latest().await;

    let before = api_counts("deregister");
    let internal_before = failure_count("deregister", "Internal");

    persistence
        .fail_writes(vec![Some(ShardManagerError::Timeout)])
        .await;
    let failed = service
        .deregister(deregister_request(
            executor(1),
            &claim_of(&state, executor(1)),
        ))
        .await
        .expect("a failed deregistration is answered, not a transport error")
        .into_inner();
    assert!(matches!(
        failed.result,
        Some(shardmanager::v1::deregister_response::Result::Failure(_))
    ));

    let released = service
        .deregister(deregister_request(
            executor(2),
            &claim_of(&state, executor(2)),
        ))
        .await
        .expect("a deregistration is answered")
        .into_inner();
    assert!(matches!(
        released.result,
        Some(shardmanager::v1::deregister_response::Result::Success(_))
    ));

    let after = api_counts("deregister");
    assert_eq!(
        after,
        ApiCounts {
            success: before.success + 1,
            failure: before.failure + 1,
            dropped: before.dropped,
        }
    );
    assert_eq!(
        failure_count("deregister", "Internal"),
        internal_before + 1,
        "a deregistration whose state write failed counts as an internal failure"
    );

    join_set.abort_all();
}
