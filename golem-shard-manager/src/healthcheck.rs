use crate::model::Pod;
use crate::worker_executor::WorkerExecutorService;
use async_trait::async_trait;
use golem_common::config::RetryConfig;
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tracing::debug;

#[async_trait]
pub trait HealthCheck {
    async fn health_check(&self, pod: &Pod) -> bool;
}

/// Executes healthcheck on all the given worker executors, and returns a set of unhealthy ones
pub async fn get_unhealthy_pods(
    health_check: Arc<dyn HealthCheck + Send + Sync>,
    pods: &HashSet<Pod>,
) -> HashSet<Pod> {
    let futures: Vec<_> = pods
        .iter()
        .map(|pod| {
            let health_check = health_check.clone();
            Box::pin(async move {
                match health_check.health_check(pod).await {
                    true => None,
                    false => Some(pod.clone()),
                }
            })
        })
        .collect();
    futures::future::join_all(futures)
        .await
        .into_iter()
        .flatten()
        .collect()
}

async fn health_check_with_retries<'a, F>(
    implementation: F,
    retry_config: &RetryConfig,
    pod: &'a Pod,
) -> bool
where
    F: Fn(&'a Pod) -> Pin<Box<dyn Future<Output = bool> + 'a + Send>>,
{
    let retry_max_attempts = retry_config.max_attempts;
    let retry_min_delay = retry_config.min_delay;
    let retry_max_delay = retry_config.max_delay;
    let retry_multiplier = retry_config.multiplier;

    let mut attempts = 0;
    let mut delay = retry_min_delay;

    loop {
        match implementation(pod).await {
            true => return true,
            false => {
                if attempts >= retry_max_attempts {
                    debug!("Health check for {pod} failed {attempts}, marking as unhealthy");
                    return false;
                }
                tokio::time::sleep(delay).await;
                attempts += 1;
                delay = std::cmp::min(delay * retry_multiplier, retry_max_delay);
            }
        }
    }
}

#[derive(Clone)]
pub struct GrpcHealthCheck {
    worker_executors: Arc<dyn WorkerExecutorService + Send + Sync>,
    retry_config: RetryConfig,
}

impl GrpcHealthCheck {
    pub fn new(
        worker_executors: Arc<dyn WorkerExecutorService + Send + Sync>,
        retry_config: RetryConfig,
    ) -> Self {
        GrpcHealthCheck {
            worker_executors,
            retry_config,
        }
    }
}

#[async_trait]
impl HealthCheck for GrpcHealthCheck {
    async fn health_check(&self, pod: &Pod) -> bool {
        health_check_with_retries(
            |pod| Box::pin(async move { self.worker_executors.health_check(pod).await }),
            &self.retry_config,
            pod,
        )
        .await
    }
}

#[cfg(feature = "kubernetes")]
pub mod kubernetes {
    use crate::healthcheck::{health_check_with_retries, HealthCheck};
    use async_trait::async_trait;
    use golem_common::config::RetryConfig;
    use k8s_openapi::api::core::v1::Pod;
    use kube::{Api, Client};
    use tracing::info;

    #[derive(Clone)]
    pub struct KubernetesHealthCheck {
        client: Client,
        namespace: String,
        retry_config: RetryConfig,
    }

    impl KubernetesHealthCheck {
        pub async fn new(
            namespace: String,
            retry_config: RetryConfig,
        ) -> Result<Self, kube::Error> {
            let client = Client::try_default().await?;
            Ok(KubernetesHealthCheck {
                client,
                namespace,
                retry_config,
            })
        }

        async fn health_check_impl(&self, pod: &crate::model::Pod) -> bool {
            let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);

            match &pod.pod_name {
                Some(pod_name) => {
                    match pods.get_opt(&pod_name).await {
                        Ok(Some(k8s_pod)) => match k8s_pod.status {
                            Some(status) => {
                                let is_ready = status
                                    .conditions
                                    .unwrap_or_default()
                                    .iter()
                                    .find(|&condition| {
                                        condition.type_ == "Ready" && condition.status == "True"
                                    })
                                    .is_some();
                                if !is_ready {
                                    info!("Pod {pod} is not ready, marking as unhealthy");
                                }
                                is_ready
                            }
                            None => {
                                info!("Pod {pod} has no status, marking as unhealthy");
                                false
                            }
                        },
                        Ok(None) => {
                            info!("Pod {pod} not found by K8s, marking as unhealthy");
                            false
                        }
                        Err(err) => {
                            info!("Error while fetching pod {pod} from K8s: {err}, marking as unhealthy");
                            false
                        }
                    }
                }
                None => {
                    info!("Pod {pod} did not provide a pod_name on registration, marking as unhealthy");
                    false
                }
            }
        }
    }

    #[async_trait]
    impl HealthCheck for KubernetesHealthCheck {
        async fn health_check(&self, pod: &crate::model::Pod) -> bool {
            health_check_with_retries(
                |pod| Box::pin(async move { self.health_check_impl(pod).await }),
                &self.retry_config,
                pod,
            )
            .await
        }
    }
}
