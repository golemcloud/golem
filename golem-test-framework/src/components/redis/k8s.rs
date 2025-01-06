// Copyright 2024-2025 Golem Cloud
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

use crate::components::k8s::{
    K8sNamespace, K8sPod, K8sRouting, K8sRoutingType, K8sService, ManagedPod, ManagedService,
    Routing,
};
use crate::components::redis::Redis;
use async_dropper_simple::AsyncDropper;
use async_trait::async_trait;
use k8s_openapi::api::core::v1::{Pod, Service};
use kube::api::PostParams;
use kube::{Api, Client};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::info;

pub struct K8sRedis {
    namespace: K8sNamespace,
    prefix: String,
    local_host: String,
    local_port: u16,
    pod: Arc<Mutex<Option<K8sPod>>>,
    service: Arc<Mutex<Option<K8sService>>>,
    routing: Arc<Mutex<Option<K8sRouting>>>,
}

impl K8sRedis {
    pub async fn new(
        namespace: &K8sNamespace,
        routing_type: &K8sRoutingType,
        prefix: String,
        timeout: Duration,
        service_annotations: Option<std::collections::BTreeMap<String, String>>,
    ) -> Self {
        info!("Creating Redis pod");

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
                "name": "golem-redis",
                "labels": {
                    "app": "golem-redis",
                    "app-group": "golem"
                },
            },
            "spec": {
                "ports": [{
                    "port": 6379,
                    "protocol": "TCP"
                }],
                "containers": [{
                    "name": "redis",
                    "image": "redis:7.2"
                }]
            }
        }))
        .expect("Failed to deserialize Pod definition");

        let pp = PostParams::default();

        let _res_pod = pods.create(&pp, &pod).await.expect("Failed to create pod");
        let managed_pod = AsyncDropper::new(ManagedPod::new("golem-redis", namespace));

        let mut service: Service = serde_json::from_value(json!({
            "apiVersion": "v1",
            "kind": "Service",
            "metadata": {
                "name": "golem-redis",
                "labels": {
                    "app": "golem-redis",
                    "app-group": "golem"
                },
            },
            "spec": {
                "ports": [{
                    "port": 6379,
                    "protocol": "TCP"
                }],
                "selector": { "app": "golem-redis" },
                "type": "LoadBalancer"
            }
        }))
        .expect("Failed to deserialize service definition");

        service.metadata.annotations = service_annotations;

        let _res_srv = services
            .create(&pp, &service)
            .await
            .expect("Failed to create service");

        let managed_service = AsyncDropper::new(ManagedService::new("golem-redis", namespace));

        let Routing {
            hostname: local_host,
            port: local_port,
            routing: managed_routing,
        } = Routing::create("golem-redis", 6379, namespace, routing_type).await;

        info!("Redis pod started, waiting for healthcheck");

        let host = format!("golem-redis.{}.svc.cluster.local", &namespace.0);
        let port = 6379;

        super::wait_for_startup(&local_host, local_port, timeout);

        info!("Redis started on private host {host}:{port}, accessible from localhost as {local_host}:{local_port}");

        K8sRedis {
            namespace: namespace.clone(),
            prefix,
            local_host,
            local_port,
            pod: Arc::new(Mutex::new(Some(managed_pod))),
            service: Arc::new(Mutex::new(Some(managed_service))),
            routing: Arc::new(Mutex::new(Some(managed_routing))),
        }
    }
}

#[async_trait]
impl Redis for K8sRedis {
    fn assert_valid(&self) {}

    fn private_host(&self) -> String {
        format!("golem-redis.{}.svc.cluster.local", &self.namespace.0)
    }

    fn private_port(&self) -> u16 {
        6379
    }

    fn public_host(&self) -> String {
        self.local_host.clone()
    }

    fn public_port(&self) -> u16 {
        self.local_port
    }

    fn prefix(&self) -> &str {
        &self.prefix
    }

    async fn kill(&self) {
        let _ = self.pod.lock().await.take();
        let _ = self.service.lock().await.take();
        let _ = self.routing.lock().await.take();
    }
}
