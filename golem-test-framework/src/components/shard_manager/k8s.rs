// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::components::k8s::{
    K8sNamespace, K8sPod, K8sRouting, K8sRoutingType, K8sService, ManagedPod, ManagedService,
    Routing,
};
use crate::components::redis::Redis;
use crate::components::shard_manager::{wait_for_startup, ShardManager};
use async_dropper_simple::AsyncDropper;
use async_trait::async_trait;
use k8s_openapi::api::core::v1::{Pod, Service};
use kube::api::PostParams;
use kube::{Api, Client};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{info, Level};

pub struct K8sShardManager {
    namespace: K8sNamespace,
    local_host: String,
    local_port: u16,
    pod: Arc<Mutex<Option<K8sPod>>>,
    service: Arc<Mutex<Option<K8sService>>>,
    routing: Arc<Mutex<Option<K8sRouting>>>,
}

impl K8sShardManager {
    const GRPC_PORT: u16 = 9020;
    const HTTP_PORT: u16 = 9021;
    const NAME: &'static str = "golem-shard-manager";

    pub async fn new(
        namespace: &K8sNamespace,
        routing_type: &K8sRoutingType,
        verbosity: Level,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        timeout: Duration,
        service_annotations: Option<std::collections::BTreeMap<String, String>>,
    ) -> Self {
        Self::new_base(
            namespace,
            routing_type,
            verbosity,
            redis,
            timeout,
            service_annotations,
        )
        .await
    }

    pub async fn new_base(
        namespace: &K8sNamespace,
        routing_type: &K8sRoutingType,
        verbosity: Level,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        timeout: Duration,
        service_annotations: Option<std::collections::BTreeMap<String, String>>,
    ) -> Self {
        info!("Starting Golem Shard Manager pod");

        let env_vars =
            super::env_vars(None, Self::HTTP_PORT, Self::GRPC_PORT, redis, verbosity).await;
        let env_vars = env_vars
            .into_iter()
            .map(|(k, v)| json!({"name": k, "value": v}))
            .collect::<Vec<_>>();

        let pods: Api<Pod> = Api::namespaced(
            Client::try_default()
                .await
                .expect("Failed to create K8s client"),
            &namespace.0,
        );
        let services: Api<Service> = Api::namespaced(
            Client::try_default()
                .await
                .expect("Failed to create K8s client"),
            &namespace.0,
        );

        let pod: Pod = serde_json::from_value(json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": {
                "name": Self::NAME,
                "labels": {
                    "app": Self::NAME,
                    "app-group": "golem"
                },
            },
            "spec": {
                "ports": [
                    {
                        "port": Self::GRPC_PORT,
                        "protocol": "TCP"
                    },
                    {
                        "port": Self::HTTP_PORT,
                        "protocol": "TCP"
                    }
                ],
                "containers": [{
                    "name": "service",
                    "image": format!("golemservices/golem-shard-manager:latest"),
                    "env": env_vars
                }]
            }
        }))
        .expect("Failed to deserialize Pod definition");

        let pp = PostParams::default();

        let _res_pod = pods.create(&pp, &pod).await.expect("Failed to create pod");
        let managed_pod = AsyncDropper::new(ManagedPod::new(Self::NAME, namespace));

        let mut service: Service = serde_json::from_value(json!({
            "apiVersion": "v1",
            "kind": "Service",
            "metadata": {
                "name": Self::NAME,
                "labels": {
                    "app": Self::NAME,
                    "app-group": "golem"
                },
            },
            "spec": {
                "ports": [
                    {
                        "name": "grpc",
                        "port": Self::GRPC_PORT,
                        "protocol": "TCP"
                    },
                    {
                        "name": "http",
                        "port": Self::HTTP_PORT,
                        "protocol": "TCP"
                    }
                ],
                "selector": { "app": Self::NAME },
                "type": "LoadBalancer"
            }
        }))
        .expect("Failed to deserialize service definition");
        service.metadata.annotations = service_annotations;

        let _res_srv = services
            .create(&pp, &service)
            .await
            .expect("Failed to create service");

        let managed_service = AsyncDropper::new(ManagedService::new(Self::NAME, namespace));

        let Routing {
            hostname: local_host,
            port: local_port,
            routing: managed_routing,
        } = Routing::create(Self::NAME, Self::GRPC_PORT, namespace, routing_type).await;

        wait_for_startup(&local_host, local_port, timeout).await;

        info!("Golem Shard Manager pod started");

        Self {
            namespace: namespace.clone(),
            local_host,
            local_port,
            pod: Arc::new(Mutex::new(Some(managed_pod))),
            service: Arc::new(Mutex::new(Some(managed_service))),
            routing: Arc::new(Mutex::new(Some(managed_routing))),
        }
    }
}

#[async_trait]
impl ShardManager for K8sShardManager {
    fn private_host(&self) -> String {
        format!("{}.{}.svc.cluster.local", Self::NAME, &self.namespace.0)
    }

    fn private_http_port(&self) -> u16 {
        Self::HTTP_PORT
    }

    fn private_grpc_port(&self) -> u16 {
        Self::GRPC_PORT
    }

    fn public_host(&self) -> String {
        self.local_host.clone()
    }

    fn public_http_port(&self) -> u16 {
        todo!()
    }

    fn public_grpc_port(&self) -> u16 {
        self.local_port
    }

    async fn kill(&self) {
        let _ = self.pod.lock().await.take();
        let _ = self.service.lock().await.take();
        let _ = self.routing.lock().await.take();
    }

    async fn restart(&self, _number_of_shards_override: Option<usize>) {
        panic!("Not supported yet");
    }
}
