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

use super::AuthServiceGrpcClient;
use super::{
    new_account_grpc_client, new_auth_grpc_client, new_project_grpc_client, new_token_grpc_client,
    AccountServiceGrpcClient, CloudService, ProjectServiceGrpcClient, TokenServiceGrpcClient,
};
use crate::components::cloud_service::wait_for_startup;
use crate::components::k8s::{
    K8sNamespace, K8sPod, K8sRouting, K8sRoutingType, K8sService, ManagedPod, ManagedService,
    Routing,
};
use crate::components::new_reqwest_client;
use crate::components::rdb::Rdb;
use crate::config::GolemClientProtocol;
use async_dropper_simple::AsyncDropper;
use async_trait::async_trait;
use k8s_openapi::api::core::v1::{Pod, Service};
use kube::api::PostParams;
use kube::{Api, Client};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, OnceCell};
use tonic::transport::Channel;
use tracing::{info, Level};

pub struct K8sCloudService {
    namespace: K8sNamespace,
    local_host: String,
    local_grpc_port: u16,
    local_http_port: u16,
    pod: Arc<Mutex<Option<K8sPod>>>,
    service: Arc<Mutex<Option<K8sService>>>,
    grpc_routing: Arc<Mutex<Option<K8sRouting>>>,
    http_routing: Arc<Mutex<Option<K8sRouting>>>,
    client_protocol: GolemClientProtocol,
    base_http_client: OnceCell<reqwest::Client>,
    account_grpc_client: OnceCell<AccountServiceGrpcClient<Channel>>,
    token_grpc_client: OnceCell<TokenServiceGrpcClient<Channel>>,
    project_grpc_client: OnceCell<ProjectServiceGrpcClient<Channel>>,
    auth_grpc_client: OnceCell<AuthServiceGrpcClient<Channel>>,
}

impl K8sCloudService {
    pub const GRPC_PORT: u16 = 9094;
    pub const HTTP_PORT: u16 = 8083;
    pub const NAME: &'static str = "cloud-service";

    pub async fn new(
        namespace: &K8sNamespace,
        routing_type: &K8sRoutingType,
        client_protocol: GolemClientProtocol,
        verbosity: Level,
        rdb: Arc<dyn Rdb>,
        timeout: Duration,
        service_annotations: Option<std::collections::BTreeMap<String, String>>,
    ) -> Self {
        info!("Starting Cloud Service pod");

        let env_vars =
            super::env_vars(Self::HTTP_PORT, Self::GRPC_PORT, rdb, verbosity, true).await;

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
                    "image": format!("golemservices/cloud-service:latest"),
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
            port: grpc_port,
            routing: grpc_routing,
        } = Routing::create(Self::NAME, Self::GRPC_PORT, namespace, routing_type).await;

        let Routing {
            port: http_port,
            routing: http_routing,
            ..
        } = Routing::create(Self::NAME, Self::HTTP_PORT, namespace, routing_type).await;

        wait_for_startup(client_protocol, &local_host, grpc_port, http_port, timeout).await;

        info!("Cloud Service pod started");

        Self {
            namespace: namespace.clone(),
            local_grpc_port: grpc_port,
            local_http_port: http_port,
            pod: Arc::new(Mutex::new(Some(managed_pod))),
            service: Arc::new(Mutex::new(Some(managed_service))),
            grpc_routing: Arc::new(Mutex::new(Some(grpc_routing))),
            http_routing: Arc::new(Mutex::new(Some(http_routing))),
            local_host,
            client_protocol,
            base_http_client: OnceCell::new(),
            account_grpc_client: OnceCell::new(),
            token_grpc_client: OnceCell::new(),
            project_grpc_client: OnceCell::new(),
            auth_grpc_client: OnceCell::new(),
        }
    }
}

#[async_trait]
impl CloudService for K8sCloudService {
    fn client_protocol(&self) -> GolemClientProtocol {
        self.client_protocol
    }

    async fn base_http_client(&self) -> reqwest::Client {
        self.base_http_client
            .get_or_init(async || new_reqwest_client())
            .await
            .clone()
    }

    async fn account_grpc_client(&self) -> AccountServiceGrpcClient<Channel> {
        self.account_grpc_client
            .get_or_init(async || {
                new_account_grpc_client(&self.public_host(), self.public_grpc_port()).await
            })
            .await
            .clone()
    }

    async fn token_grpc_client(&self) -> TokenServiceGrpcClient<Channel> {
        self.token_grpc_client
            .get_or_init(async || {
                new_token_grpc_client(&self.public_host(), self.public_grpc_port()).await
            })
            .await
            .clone()
    }

    async fn project_grpc_client(&self) -> ProjectServiceGrpcClient<Channel> {
        self.project_grpc_client
            .get_or_init(async || {
                new_project_grpc_client(&self.public_host(), self.public_grpc_port()).await
            })
            .await
            .clone()
    }

    async fn auth_grpc_client(&self) -> AuthServiceGrpcClient<Channel> {
        self.auth_grpc_client
            .get_or_init(async || {
                new_auth_grpc_client(&self.public_host(), self.public_grpc_port()).await
            })
            .await
            .clone()
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

    fn public_host(&self) -> String {
        self.local_host.clone()
    }

    fn public_http_port(&self) -> u16 {
        self.local_http_port
    }

    fn public_grpc_port(&self) -> u16 {
        self.local_grpc_port
    }

    async fn kill(&self) {
        let _ = self.pod.lock().await.take();
        let _ = self.service.lock().await.take();
        let _ = self.http_routing.lock().await.take();
        let _ = self.grpc_routing.lock().await.take();
    }
}
