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

use crate::components::component_service::ComponentService;
use crate::components::k8s::{
    K8sNamespace, K8sPod, K8sRouting, K8sRoutingType, K8sService, ManagedPod, ManagedService,
    Routing,
};
use crate::components::redis::Redis;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_executor::{
    new_client, wait_for_startup, WorkerExecutor, WorkerExecutorEnvVars,
};
use crate::components::worker_service::WorkerService;
use crate::components::GolemEnvVars;
use async_dropper_simple::AsyncDropper;
use async_trait::async_trait;
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use k8s_openapi::api::core::v1::{Pod, Service};
use kube::api::PostParams;
use kube::{Api, Client};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tonic::transport::Channel;
use tracing::{info, Level};

pub struct K8sWorkerExecutor {
    namespace: K8sNamespace,
    idx: usize,
    local_host: String,
    local_port: u16,
    pod: Arc<Mutex<Option<K8sPod>>>,
    service: Arc<Mutex<Option<K8sService>>>,
    routing: Arc<Mutex<Option<K8sRouting>>>,
    client: Option<WorkerExecutorClient<Channel>>,
}

impl K8sWorkerExecutor {
    const GRPC_PORT: u16 = 9000;
    const HTTP_PORT: u16 = 9100;

    pub async fn new(
        namespace: &K8sNamespace,
        routing_type: &K8sRoutingType,
        idx: usize,
        verbosity: Level,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        worker_service: Arc<dyn WorkerService + Send + Sync + 'static>,
        timeout: Duration,
        service_annotations: Option<std::collections::BTreeMap<String, String>>,
        shared_client: bool,
    ) -> Self {
        Self::new_base(
            Box::new(GolemEnvVars()),
            namespace,
            routing_type,
            idx,
            verbosity,
            redis,
            component_service,
            shard_manager,
            worker_service,
            timeout,
            service_annotations,
            shared_client,
        )
        .await
    }

    pub async fn new_base(
        env_vars: Box<dyn WorkerExecutorEnvVars + Send + Sync + 'static>,
        namespace: &K8sNamespace,
        routing_type: &K8sRoutingType,
        idx: usize,
        verbosity: Level,
        redis: Arc<dyn Redis + Send + Sync + 'static>,
        component_service: Arc<dyn ComponentService + Send + Sync + 'static>,
        shard_manager: Arc<dyn ShardManager + Send + Sync + 'static>,
        worker_service: Arc<dyn WorkerService + Send + Sync + 'static>,
        timeout: Duration,
        service_annotations: Option<std::collections::BTreeMap<String, String>>,
        shared_client: bool,
    ) -> Self {
        info!("Starting Golem Worker Executor {idx} pod");

        let name = &format!("golem-worker-executor-{idx}");

        let env_vars = env_vars
            .env_vars(
                Self::HTTP_PORT,
                Self::GRPC_PORT,
                component_service,
                shard_manager,
                worker_service,
                redis,
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
                "name": name,
                "labels": {
                    "app": name,
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
                    "image": "golemservices/golem-worker-executor:latest".to_string(),
                    "env": env_vars
                }]
            }
        }))
        .expect("Failed to deserialize Pod definition");

        let pp = PostParams::default();

        let _res_pod = pods.create(&pp, &pod).await.expect("Failed to create pod");
        let managed_pod = AsyncDropper::new(ManagedPod::new(name, namespace));

        let mut service: Service = serde_json::from_value(json!({
            "apiVersion": "v1",
            "kind": "Service",
            "metadata": {
                "name": name,
                "labels": {
                    "app": name,
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
                "selector": { "app": name },
                "type": "LoadBalancer"
            }
        }))
        .expect("Failed to deserialize service definition");
        service.metadata.annotations = service_annotations;

        let _res_srv = services
            .create(&pp, &service)
            .await
            .expect("Failed to create service");

        let managed_service = AsyncDropper::new(ManagedService::new(name, namespace));

        let Routing {
            hostname: local_host,
            port: local_port,
            routing: managed_routing,
        } = Routing::create(name, Self::GRPC_PORT, namespace, routing_type).await;

        wait_for_startup(&local_host, local_port, timeout).await;

        info!("Golem Worker Executor pod started");

        Self {
            namespace: namespace.clone(),
            idx,
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

    fn name(&self) -> String {
        format!("golem-worker-executor-{}", self.idx)
    }
}

#[async_trait]
impl WorkerExecutor for K8sWorkerExecutor {
    async fn client(&self) -> crate::Result<WorkerExecutorClient<Channel>> {
        match &self.client {
            Some(client) => Ok(client.clone()),
            None => Ok(new_client(&self.local_host, self.local_port).await?),
        }
    }

    fn private_host(&self) -> String {
        format!("{}.{}.svc.cluster.local", self.name(), &self.namespace.0)
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

    async fn restart(&self) {
        panic!("Not supported yet");
    }
}
