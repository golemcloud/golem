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

use anyhow::anyhow;
use async_dropper::{AsyncDrop, AsyncDropper};
use async_trait::async_trait;
use k8s_openapi::api::core::v1::{Namespace, Pod, Service};
use k8s_openapi::api::networking::v1::Ingress;
use kube::api::{DeleteParams, PostParams};
use kube::{Api, Client};
use serde_json::json;
use std::collections::BTreeMap;
use std::fmt::Display;
use std::time::Duration;
use tokio::process::{Child, Command};
use tracing::{debug, error, info};
use url::Url;

#[derive(Debug, Clone)]
pub struct K8sNamespace(pub String);

impl Default for K8sNamespace {
    fn default() -> Self {
        K8sNamespace("default".to_string())
    }
}

impl Display for K8sNamespace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone)]
pub enum K8sRoutingType {
    Minikube,
    Service,
    AlbIngress,
}

pub type K8sPod = AsyncDropper<ManagedPod>;
pub type K8sService = AsyncDropper<ManagedService>;
pub type K8sRouting = AsyncDropper<ManagedRouting>;

pub struct ManagedPod {
    name: String,
    namespace: K8sNamespace,
}

impl ManagedPod {
    pub fn new<S: Into<String>>(name: S, namespace: &K8sNamespace) -> ManagedPod {
        ManagedPod {
            name: name.into(),
            namespace: namespace.clone(),
        }
    }
}

#[async_trait]
impl AsyncDrop for ManagedPod {
    async fn async_drop(&mut self) {
        info!("Stopping pod {}", self.name);

        let name = self.name.clone();

        let pods: Api<Pod> =
            Api::namespaced(Client::try_default().await.unwrap(), &self.namespace.0);

        let dp = DeleteParams {
            grace_period_seconds: Some(0),
            ..Default::default()
        };
        match pods.delete(&name, &dp).await {
            Ok(_) => info!("Pod deleted: {name}"),
            Err(e) => error!("!!! Failed to delete pod {name}: {e:?}"),
        }
    }
}

pub struct ManagedService {
    name: String,
    namespace: K8sNamespace,
}

impl ManagedService {
    pub fn new<S: Into<String>>(name: S, namespace: &K8sNamespace) -> ManagedService {
        ManagedService {
            name: name.into(),
            namespace: namespace.clone(),
        }
    }
}

#[async_trait]
impl AsyncDrop for ManagedService {
    async fn async_drop(&mut self) {
        info!("Stopping service {}", self.name);

        let name = self.name.clone();

        let services: Api<Service> =
            Api::namespaced(Client::try_default().await.unwrap(), &self.namespace.0);

        let dp = DeleteParams {
            grace_period_seconds: Some(0),
            ..Default::default()
        };
        match services.delete(&name, &dp).await {
            Ok(_) => info!("Service deleted: {name}"),
            Err(e) => error!("!!! Failed to delete service {name}: {e:?}"),
        }
    }
}

#[allow(clippy::large_enum_variant)]
pub enum ManagedRouting {
    Minikube { child: Option<Child> },
    Ingress(ManagedIngress),
    Service,
}

impl ManagedRouting {
    pub fn minikube(child: Option<Child>) -> ManagedRouting {
        ManagedRouting::Minikube { child }
    }
    pub fn ingress<S: Into<String>>(name: S, namespace: &K8sNamespace) -> ManagedRouting {
        ManagedRouting::Ingress(ManagedIngress::new(name, namespace))
    }
    pub fn service() -> ManagedRouting {
        ManagedRouting::Service
    }
}

#[async_trait]
impl AsyncDrop for ManagedRouting {
    async fn async_drop(&mut self) {
        if let ManagedRouting::Minikube { child: Some(child) } = self {
            if let Some(pid) = child.id() {
                info!("Killing minikube service tunnel process {:?}", child.id());
                kill_tree::tokio::kill_tree(pid)
                    .await
                    .expect("Failed to kill minikube service tunnel");
            }
        }
    }
}

pub struct ManagedIngress {
    name: String,
    namespace: K8sNamespace,
}

impl ManagedIngress {
    pub fn new<S: Into<String>>(name: S, namespace: &K8sNamespace) -> ManagedIngress {
        ManagedIngress {
            name: name.into(),
            namespace: namespace.clone(),
        }
    }
}

#[async_trait]
impl AsyncDrop for ManagedIngress {
    async fn async_drop(&mut self) {
        info!("Stopping ingress {}", self.name);

        let name = self.name.clone();

        let ingresses: Api<Ingress> =
            Api::namespaced(Client::try_default().await.unwrap(), &self.namespace.0);

        let dp = DeleteParams::default();
        match ingresses.delete(&name, &dp).await {
            Ok(_) => info!("Ingress deleted: {name} in {:?}", self.namespace),
            Err(e) => error!(
                "!!! Failed to delete ingress {name} in {:?}: {e:?}",
                self.namespace
            ),
        }
    }
}

pub struct ManagedNamespace {
    namespace: K8sNamespace,
}

impl ManagedNamespace {
    pub fn new(namespace: &K8sNamespace) -> ManagedNamespace {
        ManagedNamespace {
            namespace: namespace.clone(),
        }
    }
}

#[async_trait]
impl AsyncDrop for ManagedNamespace {
    async fn async_drop(&mut self) {
        info!("Dropping namespace {}", self.namespace.0);

        let name = self.namespace.0.clone();

        let namespaces: Api<Namespace> = Api::all(Client::try_default().await.unwrap());

        let dp = DeleteParams::default();
        match namespaces.delete(&name, &dp).await {
            Ok(_) => info!("Namespace deleted: {name}"),
            Err(e) => error!(
                "!!! Failed to delete namespace {name} in {:?}: {e:?}",
                self.namespace
            ),
        }
    }
}

pub struct Routing {
    pub hostname: String,
    pub port: u16,
    pub routing: K8sRouting,
}

impl Routing {
    pub async fn create(
        service_name: &str,
        port: u16,
        namespace: &K8sNamespace,
        k8s_routing_type: &K8sRoutingType,
    ) -> Routing {
        match k8s_routing_type {
            K8sRoutingType::Minikube => Self::create_minikube_tunnel(service_name, namespace).await,
            K8sRoutingType::Service => {
                Self::create_service_route(service_name, port, namespace).await
            }
            K8sRoutingType::AlbIngress => {
                Self::create_alb_ingress(service_name, port, namespace).await
            }
        }
    }

    async fn create_minikube_tunnel(service_name: &str, namespace: &K8sNamespace) -> Routing {
        let (url, child) = Self::run_minikube_service(service_name, namespace).await;
        let hostname = url.host().expect("Can't get minikube host").to_string();

        let port = url.port().expect("Can't get port");

        Routing {
            hostname,
            port,
            routing: AsyncDropper::new(ManagedRouting::Minikube { child }),
        }
    }

    async fn create_alb_ingress(
        service_name: &str,
        port: u16,
        namespace: &K8sNamespace,
    ) -> Routing {
        info!("Creating alb ingress for service {service_name}:{port} in {namespace}");
        let ingresses: Api<Ingress> =
            Api::namespaced(Client::try_default().await.unwrap(), &namespace.0);

        let ingress: Ingress = serde_json::from_value(json!({
            "apiVersion": "networking.k8s.io/v1",
            "kind": "Ingress",
            "metadata": {
                "name": service_name,
                "labels": {
                    "app": service_name,
                    "app-group": "golem"
                },
                "annotations": {
                    "alb.ingress.kubernetes.io/scheme": "internet-facing",
                    "alb.ingress.kubernetes.io/target-type": "ip"
                }
            },
            "spec": {
                "ingressClassName": "alb",
                "rules": [
                    {
                        "http": {
                            "paths": [
                                {
                                    "backend": {
                                        "service": {
                                            "name": service_name,
                                            "port": {
                                                "number": port
                                            }
                                        }
                                    },
                                    "path": "/*",
                                    "pathType": "ImplementationSpecific"
                                }
                            ]
                        }
                    }
                ]
            }
        }))
        .expect("Failed to deserialize ingress definition");

        let pp = PostParams::default();

        let _ = ingresses
            .create(&pp, &ingress)
            .await
            .expect("Failed to create ingress");

        let routing = AsyncDropper::new(ManagedRouting::ingress(service_name, namespace));

        let hostname = Self::wait_for_load_balancer(&ingresses, service_name).await;

        Routing {
            hostname,
            port: 80,
            routing,
        }
    }

    async fn create_service_route(
        service_name: &str,
        port: u16,
        namespace: &K8sNamespace,
    ) -> Routing {
        info!("Creating route for service {service_name}:{port} in {namespace}");

        let service: Api<Service> =
            Api::namespaced(Client::try_default().await.unwrap(), &namespace.0);

        let hostname = Self::wait_for_service_load_balancer(&service, service_name).await;

        let routing = AsyncDropper::new(ManagedRouting::service());

        Routing {
            hostname,
            port,
            routing,
        }
    }

    async fn wait_for_load_balancer(ingresses: &Api<Ingress>, name: &str) -> String {
        loop {
            let res_ingress = ingresses.get(name).await.expect("Failed to get ingresses");

            match Self::ingress_hostname(&res_ingress, name) {
                Ok(hostname) => return hostname,
                Err(e) => {
                    error!("Can't get hostname for {name}: {e:?}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    fn ingress_hostname(ingress: &Ingress, service_name: &str) -> anyhow::Result<String> {
        let hostname = ingress
            .status
            .as_ref()
            .ok_or(anyhow!("No ingress status for {service_name}"))?
            .load_balancer
            .as_ref()
            .ok_or(anyhow!("No load balancer for {service_name}"))?
            .ingress
            .as_ref()
            .ok_or(anyhow!("No ingress for {service_name}"))?
            .first()
            .as_ref()
            .ok_or(anyhow!("Empty ingress for {service_name}"))?
            .hostname
            .as_ref()
            .ok_or(anyhow!("No ingress hostname for {service_name}"))?
            .to_string();

        Ok(hostname)
    }

    async fn wait_for_service_load_balancer(service: &Api<Service>, name: &str) -> String {
        loop {
            let s = service.get(name).await.expect("Failed to get ingresses");

            match Self::service_hostname(&s, name) {
                Ok(hostname) => return hostname,
                Err(e) => {
                    error!("Can't get hostname for {name}: {e:?}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    fn service_hostname(service: &Service, service_name: &str) -> anyhow::Result<String> {
        let hostname = service
            .status
            .as_ref()
            .ok_or(anyhow!("No service status for {service_name}"))?
            .load_balancer
            .as_ref()
            .ok_or(anyhow!("No load balancer for {service_name}"))?
            .ingress
            .as_ref()
            .ok_or(anyhow!("No ingress for {service_name}"))?
            .first()
            .as_ref()
            .ok_or(anyhow!("Empty ingress for {service_name}"))?
            .hostname
            .as_ref()
            .ok_or(anyhow!("No ingress hostname for {service_name}"))?
            .to_string();

        Ok(hostname)
    }

    #[cfg(not(target_os = "linux"))]
    async fn run_minikube_service(
        service_name: &str,
        namespace: &K8sNamespace,
    ) -> (Url, Option<Child>) {
        use std::process::Stdio;
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

        debug!(
            "Launching minikube service --namespace={} --url {}",
            namespace.0, service_name
        );

        let mut attempts = 0;
        loop {
            let mut child = Command::new("minikube")
                .arg("service")
                .arg(format!("--namespace={}", namespace.0))
                .arg("--url")
                .arg(service_name)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .expect("Failed to start minikube");

            let stdout = child.stdout.take().expect("Failed to get stdout");
            let mut stderr = child.stderr.take().expect("Failed to get stderr");

            let mut stdout_reader = BufReader::new(stdout).lines();

            while let Some(line) = stdout_reader
                .next_line()
                .await
                .expect("Failed to read stdout")
            {
                debug!("minikube service stdout: {}", line);
                if let Ok(url) = Url::parse(&line) {
                    return (url, Some(child));
                }
            }

            let mut stderr_string = String::new();
            stderr
                .read_to_string(&mut stderr_string)
                .await
                .expect("Failed to read stderr");

            debug!("minikube service stderr: {}", stderr_string);

            if stderr_string.contains("SVC_UNREACHABLE") && attempts < 5 {
                attempts += 1;
                tokio::time::sleep(Duration::from_secs(1)).await;
            } else {
                panic!("Failed to run minikube service for {service_name}");
            }
        }
    }

    #[cfg(target_os = "linux")]
    async fn run_minikube_service(
        service_name: &str,
        namespace: &K8sNamespace,
    ) -> (Url, Option<Child>) {
        debug!(
            "Launching minikube service --namespace={} --url {}",
            namespace.0, service_name
        );

        let mut attempts = 0;
        loop {
            let output = Command::new("minikube")
                .arg("service")
                .arg(format!("--namespace={}", namespace.0))
                .arg("--url")
                .arg(service_name)
                .output()
                .await
                .expect("Failed to start minikube");

            let stdout = std::str::from_utf8(&output.stdout).expect("Failed to parse output");
            let stderr = std::str::from_utf8(&output.stderr).expect("Failed to parse error output");

            if stderr.contains("SVC_UNREACHABLE") && attempts < 5 {
                attempts += 1;
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }

            debug!("minikube service stdout: {}", stdout);
            debug!("minikube service stderr: {}", stderr);

            let any_res = stdout
                .lines()
                .map(|l| l.trim())
                .find(|l| !l.is_empty())
                .expect("No service mapping.");

            break (Url::parse(any_res).expect("Failed to parse url"), None);
        }
    }
}

pub fn aws_nlb_service_annotations() -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    map.insert(
        "service.beta.kubernetes.io/aws-load-balancer-type".to_string(),
        "external".to_string(),
    );
    map.insert(
        "service.beta.kubernetes.io/aws-load-balancer-nlb-target-type".to_string(),
        "ip".to_string(),
    );
    map.insert(
        "service.beta.kubernetes.io/aws-load-balancer-scheme".to_string(),
        "internet-facing".to_string(),
    );
    map
}
