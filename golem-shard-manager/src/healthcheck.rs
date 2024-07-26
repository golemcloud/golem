use crate::model::Pod;
use crate::worker_executor::WorkerExecutorService;
use async_trait::async_trait;
use golem_common::config::RetryConfig;
use golem_common::retries::with_retries;
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

#[derive(thiserror::Error, Debug)]
pub enum HealthCheckError {
    #[error("gRPC: error status: {0}")]
    GrpcError(tonic::Status),
    #[error("gRPC: connect error: {0}")]
    GrpcConnectError(#[source] tonic::transport::Error),
    #[error("gRPC: {0}")]
    GrpcOther(&'static str),
    #[error("K8s: connect error: {0}")]
    K8sConnectError(#[source] kube::Error),
    #[error("K8s: {0}")]
    K8sOther(&'static str),
}

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

async fn health_check_with_retries<F>(
    target: &'static str,
    implementation: F,
    retry_config: &RetryConfig,
    pod: &Pod,
) -> bool
where
    F: for<'a> Fn(
        &'a Pod,
    ) -> Pin<Box<dyn Future<Output = Result<(), HealthCheckError>> + 'a + Send>>,
{
    with_retries(
        target,
        "healtcheck",
        Some(format!("{pod}")),
        retry_config,
        pod,
        implementation,
        |_| {
            // TODO: recheck for which errors should not be retried
            true
        },
    )
    .await
    .is_ok()
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
            "worker_executor_grpc",
            |pod| {
                let worker_executors = self.worker_executors.clone();
                Box::pin(async move { worker_executors.health_check(pod).await })
            },
            &self.retry_config,
            pod,
        )
        .await
    }
}

#[cfg(feature = "kubernetes")]
pub mod kubernetes {
    use crate::healthcheck::{health_check_with_retries, HealthCheck, HealthCheckError};
    use async_trait::async_trait;
    use golem_common::config::RetryConfig;
    use k8s_openapi::api::core::v1::{Pod, PodStatus};
    use kube::{Api, Client};

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

        async fn health_check_impl(&self, pod: &crate::model::Pod) -> Result<(), HealthCheckError> {
            let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);

            match &pod.pod_name {
                Some(pod_name) => match pods.get_opt(pod_name).await {
                    Ok(Some(k8s_pod)) => match k8s_pod.status {
                        Some(status) => Self::is_pod_ready(status)
                            .then_some(())
                            .ok_or(HealthCheckError::K8sOther("pod status is not ready")),
                        None => Err(HealthCheckError::K8sOther("no pod status")),
                    },
                    Ok(None) => Err(HealthCheckError::K8sOther("pod not found")),
                    Err(err) => Err(HealthCheckError::K8sConnectError(err)),
                },
                None => Err(HealthCheckError::K8sOther("no pod_name")),
            }
        }

        fn is_pod_ready(pod_status: PodStatus) -> bool {
            pod_status
                .conditions
                .unwrap_or_default()
                .iter()
                .any(|c| c.type_ == "Ready" && c.status == "True")
        }
    }

    #[async_trait]
    impl HealthCheck for KubernetesHealthCheck {
        async fn health_check(&self, pod: &crate::model::Pod) -> bool {
            health_check_with_retries(
                "worker_executor_k8s",
                |pod| {
                    let health_check = self.clone();
                    Box::pin(async move { health_check.health_check_impl(pod).await })
                },
                &self.retry_config,
                pod,
            )
            .await
        }
    }
}
