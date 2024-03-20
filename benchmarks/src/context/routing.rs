use crate::context::{K8sNamespace, K8sRoutingType, ManagedRouting};
use anyhow::{anyhow, Result};
use k8s_openapi::api::networking::v1::Ingress;
use kube::api::PostParams;
use kube::{Api, Client};
use serde_json::json;
use std::time::Duration;
use tokio::process::Command;
use url::Url;

pub struct Routing {
    pub hostname: String,
    pub port: u16,
    pub routing: ManagedRouting,
}

impl Routing {
    pub async fn create(
        service_name: &str,
        port: u16,
        namespace: &K8sNamespace,
        k8s_routing_type: &K8sRoutingType,
    ) -> Result<Routing> {
        match k8s_routing_type {
            K8sRoutingType::Minikube => {
                let url = Self::resolve_minikube_service(service_name, namespace).await?;
                let hostname = url.host().ok_or(anyhow!("Can't get host"))?.to_string();

                let port = url.port().ok_or(anyhow!("Can't get port"))?;

                Ok(Routing {
                    hostname,
                    port,
                    routing: ManagedRouting::Minikube,
                })
            }
            K8sRoutingType::Ingress => Self::create_ingress(service_name, port, namespace).await,
        }
    }

    async fn create_ingress(
        service_name: &str,
        port: u16,
        namespace: &K8sNamespace,
    ) -> Result<Routing> {
        println!("Creating ingress for service {service_name}:{port} in {namespace:?}");
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
        }))?;

        let pp = PostParams::default();

        let _ = ingresses.create(&pp, &ingress).await?;

        let hostname = Self::wait_for_load_balancer(&ingresses, service_name).await?;

        Ok(Routing {
            hostname,
            port: 80,
            routing: ManagedRouting::ingress(service_name, namespace),
        })
    }

    async fn wait_for_load_balancer(ingresses: &Api<Ingress>, name: &str) -> Result<String> {
        loop {
            let res_ingress = ingresses.get(name).await?;

            match Self::ingress_hostname(&res_ingress, name) {
                Ok(hostname) => return Ok(hostname),
                Err(e) => {
                    println!("Can't get hostname for {name}: {e:?}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    fn ingress_hostname(ingress: &Ingress, service_name: &str) -> Result<String> {
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
            .get(0)
            .as_ref()
            .ok_or(anyhow!("Empty ingress for {service_name}"))?
            .hostname
            .as_ref()
            .ok_or(anyhow!("No ingress hostname for {service_name}"))?
            .to_string();

        Ok(hostname)
    }

    async fn resolve_minikube_service(service_name: &str, namespace: &K8sNamespace) -> Result<Url> {
        let output = Command::new("minikube")
            .arg("service")
            .arg(&format!("--namespace={}", namespace.0))
            .arg("--url=true")
            .arg(service_name)
            .output()
            .await?;

        let res = std::str::from_utf8(&output.stdout)?;

        let any_res = res
            .lines()
            .map(|l| l.trim())
            .find(|l| !l.is_empty())
            .ok_or(anyhow!("No service mapping."))?;

        Ok(Url::parse(any_res)?)
    }
}
