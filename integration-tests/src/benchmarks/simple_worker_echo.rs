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

use std::sync::Arc;
use tokio::io::{stdin, AsyncReadExt};
use tracing::Level;

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

use golem_test_framework::components::k8s::{K8sNamespace, K8sRoutingType};
use golem_test_framework::components::rdb::k8s_postgres::K8sPostgresRdb;
use golem_test_framework::components::rdb::Rdb;
use golem_test_framework::components::redis::k8s::K8sRedis;
use golem_test_framework::components::redis::Redis;
use golem_test_framework::components::shard_manager::k8s::K8sShardManager;
use golem_test_framework::components::shard_manager::ShardManager;

#[tokio::main]
async fn main() {
    let ansi_layer = tracing_subscriber::fmt::layer()
        .with_ansi(true)
        .with_filter(
            EnvFilter::try_new("debug,cranelift_codegen=warn,wasmtime_cranelift=warn,wasmtime_jit=warn,h2=warn,hyper=warn,tower=warn,fred=warn").unwrap()
        );

    tracing_subscriber::registry().with(ansi_layer).init();

    let namespace = K8sNamespace::default();
    let routing_type = K8sRoutingType::Minikube;

    let redis: Arc<dyn Redis + Send + Sync + 'static> =
        Arc::new(K8sRedis::new(&namespace, &routing_type, "".to_string()).await);
    let rdb: Arc<dyn Rdb + Send + Sync + 'static> =
        Arc::new(K8sPostgresRdb::new(&namespace, &routing_type).await);
    let shard_manager: Arc<dyn ShardManager + Send + Sync + 'static> = Arc::new(
        K8sShardManager::new(&namespace, &routing_type, Level::DEBUG, redis.clone()).await,
    );

    println!("Hello, world!");

    let mut answer = String::new();
    stdin().read_to_string(&mut answer).await;
    // panic!("nemjo");
}
