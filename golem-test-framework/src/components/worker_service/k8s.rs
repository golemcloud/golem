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

use crate::components::component_service::ComponentService;
use crate::components::k8s::{
    K8sNamespace, K8sPod, K8sRouting, K8sRoutingType, K8sService, ManagedPod, ManagedService,
    Routing,
};
use crate::components::rdb::Rdb;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_service::{
    new_api_definition_client, new_api_deployment_client, new_api_security_client,
    new_worker_client, wait_for_startup, ApiDefinitionServiceClient, ApiDeploymentServiceClient,
    ApiSecurityServiceClient, WorkerService, WorkerServiceClient,
};
use crate::config::GolemClientProtocol;
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

use super::WorkerServiceInternal;

pub struct K8sWorkerService {
    namespace: K8sNamespace,
    local_host: String,
    local_grpc_port: u16,
    local_http_port: u16,
    pod: Arc<Mutex<Option<K8sPod>>>,
    service: Arc<Mutex<Option<K8sService>>>,
    grpc_routing: Arc<Mutex<Option<K8sRouting>>>,
    http_routing: Arc<Mutex<Option<K8sRouting>>>,
    client_protocol: GolemClientProtocol,
    worker_client: WorkerServiceClient,
    api_definition_client: ApiDefinitionServiceClient,
    api_deployment_client: ApiDeploymentServiceClient,
    api_security_client: ApiSecurityServiceClient,
    component_service: Arc<dyn ComponentService>,
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
        component_service: Arc<dyn ComponentService>,
        shard_manager: Arc<dyn ShardManager + Send + Sync>,
        rdb: Arc<dyn Rdb + Send + Sync>,
        timeout: Duration,
        service_annotations: Option<std::collections::BTreeMap<String, String>>,
        client_protocol: GolemClientProtocol,
    ) -> Self {
        info!("Starting Golem Worker Service pod");

        let env_vars = super::env_vars(
            Self::HTTP_PORT,
            Self::GRPC_PORT,
            Self::CUSTOM_REQUEST_PORT,
            &component_service,
            &shard_manager,
            &rdb,
            verbosity,
            true,
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

        let grpc_routing =
            Routing::create(Self::NAME, Self::GRPC_PORT, namespace, routing_type).await;
        let http_routing =
            Routing::create(Self::NAME, Self::HTTP_PORT, namespace, routing_type).await;

        wait_for_startup(
            client_protocol,
            &grpc_routing.hostname,
            grpc_routing.port,
            http_routing.port,
            timeout,
        )
        .await;

        info!("Golem Worker Service pod started");

        Self {
            namespace: namespace.clone(),
            local_host: grpc_routing.hostname.clone(),
            local_grpc_port: grpc_routing.port,
            local_http_port: http_routing.port,
            pod: Arc::new(Mutex::new(Some(managed_pod))),
            service: Arc::new(Mutex::new(Some(managed_service))),
            grpc_routing: Arc::new(Mutex::new(Some(grpc_routing.routing))),
            http_routing: Arc::new(Mutex::new(Some(http_routing.routing))),
            client_protocol,
            worker_client: new_worker_client(
                client_protocol,
                &grpc_routing.hostname,
                grpc_routing.port,
                http_routing.port,
            )
            .await,
            api_definition_client: new_api_definition_client(
                client_protocol,
                &grpc_routing.hostname,
                grpc_routing.port,
                http_routing.port,
            )
            .await,
            api_deployment_client: new_api_deployment_client(
                client_protocol,
                &grpc_routing.hostname,
                grpc_routing.port,
                http_routing.port,
            )
            .await,
            api_security_client: new_api_security_client(
                client_protocol,
                &grpc_routing.hostname,
                grpc_routing.port,
                http_routing.port,
            )
            .await,
            component_service: component_service.clone(),
        }
    }
}

impl WorkerServiceInternal for K8sWorkerService {
    fn client_protocol(&self) -> GolemClientProtocol {
        self.client_protocol
    }

    fn worker_client(&self) -> WorkerServiceClient {
        self.worker_client.clone()
    }

    fn api_definition_client(&self) -> ApiDefinitionServiceClient {
        self.api_definition_client.clone()
    }

    fn api_deployment_client(&self) -> ApiDeploymentServiceClient {
        self.api_deployment_client.clone()
    }

    fn api_security_client(&self) -> ApiSecurityServiceClient {
        self.api_security_client.clone()
    }

    fn component_service(&self) -> &Arc<dyn ComponentService> {
        &self.component_service
    }
}

#[async_trait]
impl WorkerService for K8sWorkerService {
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
        self.local_http_port
    }

    fn public_grpc_port(&self) -> u16 {
        self.local_grpc_port
    }

    fn public_custom_request_port(&self) -> u16 {
        todo!()
    }

    async fn kill(&self) {
        let _ = self.pod.lock().await.take();
        let _ = self.service.lock().await.take();
        let _ = self.grpc_routing.lock().await.take();
        let _ = self.http_routing.lock().await.take();
    }
}
