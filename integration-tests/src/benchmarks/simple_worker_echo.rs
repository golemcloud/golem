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

use tokio::io::{stdin, AsyncReadExt};

use golem_test_framework::config::CliTestDependencies;

#[tokio::main]
async fn main() {
    let _deps = CliTestDependencies::new().await;
    //
    //
    // let namespace = K8sNamespace::default();
    // let routing_type = K8sRoutingType::Minikube;
    //
    // let redis: Arc<dyn Redis + Send + Sync + 'static> =
    //     Arc::new(K8sRedis::new(&namespace, &routing_type, "".to_string()).await);
    // let rdb: Arc<dyn Rdb + Send + Sync + 'static> =
    //     Arc::new(K8sPostgresRdb::new(&namespace, &routing_type).await);
    // let shard_manager: Arc<dyn ShardManager + Send + Sync + 'static> = Arc::new(
    //     K8sShardManager::new(&namespace, &routing_type, Level::DEBUG, redis.clone()).await,
    // );
    //
    println!("Hello, world!");

    let mut answer = String::new();
    stdin().read_to_string(&mut answer).await;
    // // panic!("nemjo");
}
