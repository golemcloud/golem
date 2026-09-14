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
mod persistence;
mod proxy;

use golem_test_framework::components::etcd::docker_etcd::DockerEtcd;
use std::sync::Arc;
use test_r::inherit_test_dep;

inherit_test_dep!(Arc<DockerEtcd>);
