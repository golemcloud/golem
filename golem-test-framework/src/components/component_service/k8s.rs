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

use super::ComponentServiceGrpcClient;
use super::PluginServiceGrpcClient;
use super::{new_component_grpc_client, new_plugin_grpc_client};
use crate::components::cloud_service::CloudService;
use crate::components::component_service::{wait_for_startup, ComponentService};
use crate::components::k8s::{
    K8sNamespace, K8sPod, K8sRouting, K8sRoutingType, K8sService, ManagedPod, ManagedService,
    Routing,
};
use crate::components::new_reqwest_client;
use crate::components::rdb::Rdb;
use crate::config::GolemClientProtocol;
use async_dropper_simple::AsyncDropper;
use async_trait::async_trait;
use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use k8s_openapi::api::core::v1::{Pod, Service};
use kube::api::PostParams;
use kube::{Api, Client};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, OnceCell};
use tonic::transport::Channel;
use tracing::{info, Level};

pub struct K8sComponentService {
    component_directory: PathBuf,
    namespace: K8sNamespace,
    local_host: String,
    local_grpc_port: u16,
    local_http_port: u16,
    pod: Arc<Mutex<Option<K8sPod>>>,
    service: Arc<Mutex<Option<K8sService>>>,
    grpc_routing: Arc<Mutex<Option<K8sRouting>>>,
    http_routing: Arc<Mutex<Option<K8sRouting>>>,
    plugin_wasm_files_service: Arc<PluginWasmFilesService>,
    cloud_service: Arc<dyn CloudService>,
    client_protocol: GolemClientProtocol,
    base_http_client: OnceCell<reqwest::Client>,
    component_grpc_client: OnceCell<ComponentServiceGrpcClient<Channel>>,
    plugin_grpc_client: OnceCell<PluginServiceGrpcClient<Channel>>,
}

impl K8sComponentService {
    const GRPC_PORT: u16 = 9091;
    const HTTP_PORT: u16 = 8081;
    const NAME: &'static str = "golem-component-service";

    pub async fn new(
        component_directory: PathBuf,
        namespace: &K8sNamespace,
        routing_type: &K8sRoutingType,
        verbosity: Level,
        component_compilation_service: Option<(&str, u16)>,
        rdb: Arc<dyn Rdb>,
        timeout: Duration,
        service_annotations: Option<std::collections::BTreeMap<String, String>>,
        client_protocol: GolemClientProtocol,
        plugin_wasm_files_service: Arc<PluginWasmFilesService>,
        cloud_service: Arc<dyn CloudService>,
    ) -> Self {
        info!("Starting Golem Component Service pod");

        let env_vars = super::env_vars(
            Self::HTTP_PORT,
            Self::GRPC_PORT,
            component_compilation_service,
            rdb,
            verbosity,
            true,
            &cloud_service,
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
                    }
                ],
                "containers": [{
                    "name": "service",
                    "image": "golemservices/golem-component-service:latest".to_string(),
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

        info!("Golem Component Service pod started");

        Self {
            component_directory,
            local_host: grpc_routing.hostname.clone(),
            local_grpc_port: grpc_routing.port,
            local_http_port: http_routing.port,
            namespace: namespace.clone(),
            pod: Arc::new(Mutex::new(Some(managed_pod))),
            service: Arc::new(Mutex::new(Some(managed_service))),
            grpc_routing: Arc::new(Mutex::new(Some(grpc_routing.routing))),
            http_routing: Arc::new(Mutex::new(Some(http_routing.routing))),
            plugin_wasm_files_service,
            cloud_service,
            client_protocol,
            base_http_client: OnceCell::new(),
            component_grpc_client: OnceCell::new(),
            plugin_grpc_client: OnceCell::new(),
        }
    }
}

#[async_trait]
impl ComponentService for K8sComponentService {
    fn component_directory(&self) -> &Path {
        &self.component_directory
    }

    fn cloud_service(&self) -> Arc<dyn CloudService> {
        self.cloud_service.clone()
    }

    fn plugin_wasm_files_service(&self) -> Arc<PluginWasmFilesService> {
        self.plugin_wasm_files_service.clone()
    }

    fn client_protocol(&self) -> GolemClientProtocol {
        self.client_protocol
    }

    async fn base_http_client(&self) -> reqwest::Client {
        self.base_http_client
            .get_or_init(async || new_reqwest_client())
            .await
            .clone()
    }

    async fn component_grpc_client(&self) -> ComponentServiceGrpcClient<Channel> {
        self.component_grpc_client
            .get_or_init(async || {
                new_component_grpc_client(&self.public_host(), self.public_grpc_port()).await
            })
            .await
            .clone()
    }

    async fn plugin_grpc_client(&self) -> PluginServiceGrpcClient<Channel> {
        self.plugin_grpc_client
            .get_or_init(async || {
                new_plugin_grpc_client(&self.public_host(), self.public_grpc_port()).await
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
