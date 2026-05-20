// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use super::error::HealthCheckError;
use super::worker_executor::WorkerExecutorService;
use async_trait::async_trait;
use golem_common::model::{Pod, RetryConfig};
use golem_common::retriable_error::IsRetriableError;
use golem_common::retries::with_retries_customized;
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

#[async_trait]
pub trait HealthCheck: Send + Sync {
    async fn health_check(&self, pod: Pod, pod_name: Option<String>) -> bool;
}

/// Executes healthcheck on all the given worker executors, and returns a set of unhealthy ones
pub async fn get_unhealthy_pods(
    health_check: &Arc<dyn HealthCheck>,
    pods: &[(Pod, Option<String>)],
) -> HashSet<Pod> {
    let futures: Vec<_> = pods
        .iter()
        .map(|(pod, pod_name)| {
            let health_check = health_check.clone();
            Box::pin(async move {
                match health_check.health_check(*pod, pod_name.clone()).await {
                    true => None,
                    false => Some(*pod),
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
    pod: Pod,
    pod_name: Option<String>,
    silent: bool,
) -> bool
where
    F: for<'a> Fn(
        &'a (Pod, Option<String>),
    ) -> Pin<Box<dyn Future<Output = Result<(), HealthCheckError>> + 'a + Send>>,
{
    with_retries_customized(
        target,
        "healtcheck",
        Some(format!("{pod}")),
        retry_config,
        &(pod, pod_name),
        implementation,
        IsRetriableError::is_retriable,
        IsRetriableError::as_loggable,
        silent,
    )
    .await
    .is_ok()
}

#[derive(Clone)]
pub struct GrpcHealthCheck {
    worker_executors: Arc<dyn WorkerExecutorService + Send + Sync>,
    retry_config: RetryConfig,
    silent: bool,
}

impl GrpcHealthCheck {
    pub fn new(
        worker_executors: Arc<dyn WorkerExecutorService + Send + Sync>,
        retry_config: RetryConfig,
        silent: bool,
    ) -> Self {
        GrpcHealthCheck {
            worker_executors,
            retry_config,
            silent,
        }
    }
}

#[async_trait]
impl HealthCheck for GrpcHealthCheck {
    async fn health_check(&self, pod: Pod, pod_name: Option<String>) -> bool {
        health_check_with_retries(
            "worker_executor_grpc",
            |(pod, _)| {
                let worker_executors = self.worker_executors.clone();
                Box::pin(async move { worker_executors.health_check(pod).await })
            },
            &self.retry_config,
            pod,
            pod_name,
            self.silent,
        )
        .await
    }
}

#[cfg(feature = "kubernetes")]
pub mod kubernetes {
    use super::{HealthCheck, HealthCheckError, health_check_with_retries};
    use async_trait::async_trait;
    use golem_common::model::RetryConfig;
    use k8s_openapi::api::core::v1::{Pod, PodStatus};
    use kube::{Api, Client};

    #[derive(Clone)]
    pub struct KubernetesHealthCheck {
        client: Client,
        namespace: String,
        retry_config: RetryConfig,
        silent: bool,
    }

    impl KubernetesHealthCheck {
        pub async fn new(
            namespace: String,
            retry_config: RetryConfig,
            silent: bool,
        ) -> Result<Self, kube::Error> {
            let client = Client::try_default().await?;
            Ok(KubernetesHealthCheck {
                client,
                namespace,
                retry_config,
                silent,
            })
        }

        async fn health_check_impl(
            &self,
            pod_name: Option<&String>,
        ) -> Result<(), HealthCheckError> {
            let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.namespace);

            match pod_name {
                Some(pod_name) => match pods.get_opt(pod_name).await {
                    Ok(Some(k8s_pod)) => Self::check_pod(k8s_pod),
                    Ok(None) => Err(HealthCheckError::K8sPodNotFound),
                    Err(err) => Err(HealthCheckError::K8sConnectError(err)),
                },
                None => Err(HealthCheckError::K8sNoPodName),
            }
        }

        fn check_pod(pod: Pod) -> Result<(), HealthCheckError> {
            let status = pod.status.ok_or(HealthCheckError::K8sNoPodStatus)?;
            match status.phase.as_deref() {
                Some("Failed" | "Succeeded") => Err(HealthCheckError::K8sPodTerminated),
                _ => Self::is_pod_ready(status)
                    .then_some(())
                    .ok_or(HealthCheckError::K8sPodNotReady),
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

    #[cfg(test)]
    mod tests {
        use super::KubernetesHealthCheck;
        use crate::sharding::error::HealthCheckError;
        use k8s_openapi::api::core::v1::{Pod, PodCondition, PodStatus};
        use test_r::test;

        fn pod(status: Option<PodStatus>) -> Pod {
            Pod {
                status,
                ..Default::default()
            }
        }

        fn pod_status(phase: &str, ready: bool) -> PodStatus {
            PodStatus {
                phase: Some(phase.to_string()),
                conditions: Some(vec![PodCondition {
                    type_: "Ready".to_string(),
                    status: if ready { "True" } else { "False" }.to_string(),
                    ..Default::default()
                }]),
                ..Default::default()
            }
        }

        #[test]
        fn failed_and_succeeded_pods_are_terminal() {
            assert!(matches!(
                KubernetesHealthCheck::check_pod(pod(Some(pod_status("Failed", false)))),
                Err(HealthCheckError::K8sPodTerminated)
            ));
            assert!(matches!(
                KubernetesHealthCheck::check_pod(pod(Some(pod_status("Succeeded", false)))),
                Err(HealthCheckError::K8sPodTerminated)
            ));
        }

        #[test]
        fn running_not_ready_pod_is_transient() {
            assert!(matches!(
                KubernetesHealthCheck::check_pod(pod(Some(pod_status("Running", false)))),
                Err(HealthCheckError::K8sPodNotReady)
            ));
        }
    }

    #[async_trait]
    impl HealthCheck for KubernetesHealthCheck {
        async fn health_check(
            &self,
            pod: golem_common::model::Pod,
            pod_name: Option<String>,
        ) -> bool {
            health_check_with_retries(
                "worker_executor_k8s",
                |(_, pod_name)| {
                    let health_check = self.clone();
                    Box::pin(async move { health_check.health_check_impl(pod_name.as_ref()).await })
                },
                &self.retry_config,
                pod,
                pod_name,
                self.silent,
            )
            .await
        }
    }
}
