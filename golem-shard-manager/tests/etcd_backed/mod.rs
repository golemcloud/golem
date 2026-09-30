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

//! Modules sharing the per-worker etcd server and the fixed `STATE_KEY`. `lib.rs` runs them as
//! one sequential suite.

mod distributed_startup;
mod leader_election;
pub(crate) mod persistence;
mod proxy;
mod service;

use crate::etcd_backed::persistence::GetRoutingTablePersistence;
use golem_test_framework::components::etcd::docker_etcd::DockerEtcd;
use std::sync::Arc;
use test_r::{inherit_test_dep, test_dep};

inherit_test_dep!(Arc<DockerEtcd>);

// The persistence-backend fixtures live here rather than in one of the modules below, because
// `define_matrix_dimension!` emits a module-local helper: a dimension can only be declared in the
// module whose tests use it, and every such module inherits these deps from their common parent.
// Their constructors are in `persistence`.

#[test_dep(scope = Shared, tagged_as = "sqlite")]
async fn sqlite_persistence() -> Arc<dyn GetRoutingTablePersistence> {
    persistence::sqlite_persistence().await
}

#[test_dep(scope = Shared, tagged_as = "postgres")]
async fn postgres_persistence() -> Arc<dyn GetRoutingTablePersistence> {
    persistence::postgres_persistence().await
}

#[test_dep(scope = PerWorker, tagged_as = "etcd")]
async fn etcd_persistence(etcd: &Arc<DockerEtcd>) -> Arc<dyn GetRoutingTablePersistence> {
    persistence::etcd_persistence(etcd).await
}
