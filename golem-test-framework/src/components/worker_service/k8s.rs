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

use crate::components::component_service::ComponentService;
use crate::components::k8s::{
    K8sNamespace, K8sPod, K8sRouting, K8sRoutingType, K8sService, ManagedPod, ManagedService,
    Routing,
};
use crate::components::rdb::Rdb;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_service::{
    new_client, wait_for_startup, WorkerService, WorkerServiceEnvVars,
};
use crate::components::GolemEnvVars;
use async_dropper_simple::AsyncDropper;
use async_trait::async_trait;
use golem_api_grpc::proto::golem::worker::v1::worker_service_client::WorkerServiceClient;
use k8s_openapi::api::core::v1::{Pod, Service};
use kube::api::PostParams;
use kube::{Api, Client};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tonic::transport::Channel;
use tracing::{info, Level};

pub struct K8sWorkerService {
    namespace: K8sNamespace,
    local_host: String,
    local_port: u16,
    pod: Arc<Mutex<Option<K8sPod>>>,
    service: Arc<Mutex<Option<K8sService>>>,
    routing: Arc<Mutex<Option<K8sRouting>>>,
    client: Option<WorkerServiceClient<Channel>>,
}

impl K8sWorkerService {
    const GRPC_PORT: u16 = 9092;
    const HTTP_PORT: u16 = 8082;
    const CUSTOM_REQUEST_PORT: u16 = 9093;
    const NAME: &'static str = "golem-worker-service";

    pub async fn new(
        namespace: &K8sNamespace,
        routing_type: &K8sRoutingType,
        verbosity: Level,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        rdb: Arc<dyn Rdb + Send + Sync + 'static>,
        timeout: Duration,
        service_annotations: Option<std::collections::BTreeMap<String, String>>,
        shared_client: bool,
    ) -> Self {
        Self::new_base(
            Box::new(GolemEnvVars()),
            namespace,
            routing_type,
            verbosity,
            component_service,
            shard_manager,
            rdb,
            timeout,
            service_annotations,
            shared_client,
        )
        .await
    }

    pub async fn new_base(
        env_vars: Box<dyn WorkerServiceEnvVars + Send + Sync + 'static>,
        namespace: &K8sNamespace,
        routing_type: &K8sRoutingType,
        verbosity: Level,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        rdb: Arc<dyn Rdb + Send + Sync + 'static>,
        timeout: Duration,
        service_annotations: Option<std::collections::BTreeMap<String, String>>,
        shared_client: bool,
    ) -> Self {
        info!("Starting Golem Worker Service pod");

        let env_vars = env_vars
            .env_vars(
                Self::HTTP_PORT,
                Self::GRPC_PORT,
                Self::CUSTOM_REQUEST_PORT,
                component_service,
                shard_manager,
                rdb,
                verbosity,
            )
            .await;
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
                    },
                    {
                        "port": Self::CUSTOM_REQUEST_PORT,
                        "protocol": "TCP"
                    }
                ],
                "containers": [{
                    "name": "service",
                    "image": format!("golemservices/golem-worker-service:latest"),
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
                    },
                    {
                        "name": "custom-request",
                        "port": Self::CUSTOM_REQUEST_PORT,
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

        info!("Golem Worker Service pod started");

        Self {
            namespace: namespace.clone(),
            local_host: local_host.clone(),
            local_port,
            pod: Arc::new(Mutex::new(Some(managed_pod))),
            service: Arc::new(Mutex::new(Some(managed_service))),
            routing: Arc::new(Mutex::new(Some(managed_routing))),
            client: if shared_client {
                Some(
                    new_client(&local_host, local_port)
                        .await
                        .expect("Failed to create client"),
                )
            } else {
                None
            },
        }
    }
}

#[async_trait]
impl WorkerService for K8sWorkerService {
    async fn client(&self) -> crate::Result<WorkerServiceClient<Channel>> {
        match &self.client {
            Some(client) => Ok(client.clone()),
            None => Ok(new_client(&self.local_host, self.local_port).await?),
        }
    }

    fn private_host(&self) -> String {
        format!("{}.{}.svc.cluster.local", Self::NAME, &self.namespace.0)
    }

    fn private_http_port(&self) -> u16 {
        Self::HTTP_PORT
    }

    fn private_grpc_port(&self) -> u16 {
        Self::GRPC_PORT
    }

    fn private_custom_request_port(&self) -> u16 {
        todo!()
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

    fn public_custom_request_port(&self) -> u16 {
        todo!()
    }

    async fn kill(&self) {
        let _ = self.pod.lock().await.take();
        let _ = self.service.lock().await.take();
        let _ = self.routing.lock().await.take();
    }
}
