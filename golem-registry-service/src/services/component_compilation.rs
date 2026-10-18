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

use crate::config::{ComponentCompilationConfig, ComponentCompilationEnabledConfig};
use async_trait::async_trait;
use golem_api_grpc::proto::golem::componentcompilation::v1::{
    ComponentCompilationRequest, component_compilation_error, component_compilation_response,
    component_compilation_service_client::ComponentCompilationServiceClient,
};
use golem_common::base_model::api::error_code::INTERNAL_COMPILATION_QUEUE_FULL;
use golem_common::model::component::ComponentId;
use golem_common::model::component::ComponentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::grpc::client::GrpcClient;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tonic_tracing_opentelemetry::middleware::client::OtelGrpcService;

#[async_trait]
pub trait ComponentCompilationService: Debug + Send + Sync {
    async fn enqueue_compilation(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    );

    fn set_own_grpc_port(&self, grpc_port: u16);
}

pub fn configured(config: &ComponentCompilationConfig) -> Arc<dyn ComponentCompilationService> {
    match config {
        ComponentCompilationConfig::Disabled(_) => Arc::new(DisabledComponentCompilationService),
        ComponentCompilationConfig::Enabled(inner) => {
            Arc::new(GrpcComponentCompilationService::new(inner))
        }
    }
}

pub struct GrpcComponentCompilationService {
    client: GrpcClient<ComponentCompilationServiceClient<OtelGrpcService<Channel>>>,
    own_grpc_port: AtomicU16,
    enqueue_timeout: Duration,
}

impl GrpcComponentCompilationService {
    pub fn new(config: &ComponentCompilationEnabledConfig) -> Self {
        let client = GrpcClient::new(
            "component-compilation-service",
            |channel, max_message_size| {
                ComponentCompilationServiceClient::new(channel)
                    .send_compressed(CompressionEncoding::Gzip)
                    .accept_compressed(CompressionEncoding::Gzip)
                    .max_decoding_message_size(max_message_size)
                    .max_encoding_message_size(max_message_size)
            },
            config.uri(),
            config.client_config.clone(),
        );
        Self {
            client,
            own_grpc_port: AtomicU16::new(0),
            enqueue_timeout: config.enqueue_timeout,
        }
    }
}

impl Debug for GrpcComponentCompilationService {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentCompilationServiceDefault")
            .finish()
    }
}

#[async_trait]
impl ComponentCompilationService for GrpcComponentCompilationService {
    async fn enqueue_compilation(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) {
        let component_service_port = match self.own_grpc_port.load(Ordering::Acquire) {
            0 => None,
            port => Some(port as u32),
        };

        let call = self.client.call("enqueue-compilation", move |client| {
            Box::pin(async move {
                let request = ComponentCompilationRequest {
                    component_id: Some(component_id.into()),
                    component_revision: component_revision.into(),
                    component_service_port,
                    environment_id: Some(environment_id.into()),
                };

                client.enqueue_compilation(request).await
            })
        });
        let outcome = match tokio::time::timeout(self.enqueue_timeout, call).await {
            Ok(Ok(response)) => match response.into_inner().result {
                Some(component_compilation_response::Result::Success(_)) => {
                    tracing::debug!(%component_id, %component_revision, "Enqueued compilation of uploaded component");
                    "accepted"
                }
                Some(component_compilation_response::Result::Failure(error)) => {
                    let rejected = matches!(
                        &error.error,
                        Some(component_compilation_error::Error::InternalError(body))
                            if body.code == INTERNAL_COMPILATION_QUEUE_FULL
                    );
                    tracing::warn!(%component_id, %component_revision, error = ?error, "Skipped precompilation of uploaded component");
                    if rejected { "rejected" } else { "failed" }
                }
                None => {
                    tracing::warn!(%component_id, %component_revision, "Compilation service returned an empty response");
                    "failed"
                }
            },
            Ok(Err(error)) => {
                tracing::warn!(%component_id, %component_revision, error = ?error, "Failed to enqueue compilation");
                "failed"
            }
            Err(_) => {
                tracing::warn!(%component_id, %component_revision, timeout = ?self.enqueue_timeout, "Compilation enqueue timed out; skipping precompilation");
                "timeout"
            }
        };
        crate::metrics::COMPILATION_ENQUEUE_TOTAL
            .with_label_values(&[outcome])
            .inc();
    }

    fn set_own_grpc_port(&self, grpc_port: u16) {
        self.own_grpc_port.store(grpc_port, Ordering::Release);
    }
}

pub struct DisabledComponentCompilationService;

impl Debug for DisabledComponentCompilationService {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentCompilationServiceDisabled")
            .finish()
    }
}

#[async_trait]
impl ComponentCompilationService for DisabledComponentCompilationService {
    async fn enqueue_compilation(
        &self,
        _environment_id: EnvironmentId,
        _component_id: ComponentId,
        _component_revision: ComponentRevision,
    ) {
    }
    fn set_own_grpc_port(&self, _grpc_port: u16) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_api_grpc::proto::golem::common::ErrorBody;
    use golem_api_grpc::proto::golem::componentcompilation::v1::{
        ComponentCompilationError, ComponentCompilationResponse,
        component_compilation_service_server::{
            ComponentCompilationService as GrpcService, ComponentCompilationServiceServer,
        },
    };
    use golem_common::model::RetryConfig;
    use golem_schema::proto::golem::common::Empty;
    use test_r::{test, timeout};
    use tokio::net::TcpListener;
    use tokio::task::JoinSet;
    use tokio_stream::wrappers::TcpListenerStream;
    use tonic::{Request, Response, Status};

    struct CompilationServer;

    #[async_trait]
    impl GrpcService for CompilationServer {
        async fn enqueue_compilation(
            &self,
            request: Request<ComponentCompilationRequest>,
        ) -> Result<Response<ComponentCompilationResponse>, Status> {
            let result = match request.into_inner().component_revision {
                0 => Some(component_compilation_response::Result::Success(Empty {})),
                revision @ (1 | 2) => Some(component_compilation_response::Result::Failure(
                    ComponentCompilationError {
                        error: Some(component_compilation_error::Error::InternalError(
                            ErrorBody {
                                code: if revision == 1 {
                                    INTERNAL_COMPILATION_QUEUE_FULL
                                } else {
                                    "INTERNAL_UNKNOWN"
                                }
                                .to_owned(),
                                error: "not admitted".to_owned(),
                            },
                        )),
                    },
                )),
                3 => None,
                4 => return Err(Status::permission_denied("test rejection")),
                5 => return std::future::pending().await,
                6 => return Err(Status::unavailable("test retry")),
                _ => unreachable!(),
            };
            Ok(Response::new(ComponentCompilationResponse { result }))
        }
    }

    #[test]
    #[timeout("30s")]
    async fn enqueue_is_best_effort_and_bounds_the_entire_attempt() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut tasks = JoinSet::new();
        let mut config = ComponentCompilationEnabledConfig {
            host: "127.0.0.1".to_owned(),
            port: listener.local_addr().unwrap().port(),
            enqueue_timeout: Duration::from_millis(500),
            ..Default::default()
        };
        // Each retry delay exceeds the total budget. A per-RPC timeout cannot bound this.
        config.client_config.retries_on_unavailable = RetryConfig {
            max_attempts: 100,
            min_delay: Duration::from_secs(5),
            max_delay: Duration::from_secs(5),
            multiplier: 1.0,
            max_jitter_factor: None,
        };
        tasks.spawn(
            tonic::transport::Server::builder()
                .add_service(
                    ComponentCompilationServiceServer::new(CompilationServer)
                        .accept_compressed(CompressionEncoding::Gzip)
                        .send_compressed(CompressionEncoding::Gzip),
                )
                .serve_with_incoming(TcpListenerStream::new(listener)),
        );
        let service = GrpcComponentCompilationService::new(&config);
        for (revision, outcome) in [
            (0, "accepted"),
            (1, "rejected"),
            (2, "failed"),
            (3, "failed"),
            (4, "failed"),
            (5, "timeout"),
            (6, "timeout"),
        ] {
            let outcomes = ["accepted", "rejected", "failed", "timeout"];
            let counters = outcomes
                .map(|label| crate::metrics::COMPILATION_ENQUEUE_TOTAL.with_label_values(&[label]));
            let before = counters.each_ref().map(|counter| counter.get());
            tokio::time::timeout(
                Duration::from_millis(900),
                service.enqueue_compilation(
                    EnvironmentId::new(),
                    ComponentId::new(),
                    ComponentRevision::new(revision).unwrap(),
                ),
            )
            .await
            .expect("cache warming must not hold up an upload");
            for ((label, counter), before) in outcomes.into_iter().zip(counters).zip(before) {
                assert_eq!(
                    counter.get(),
                    before + u64::from(label == outcome),
                    "wrong {label} count for revision {revision}"
                );
            }
        }

        // A peer that accepts TCP but never serves HTTP/2 must also fit the same budget.
        let stalled_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        config.port = stalled_listener.local_addr().unwrap().port();
        let stalled_service = GrpcComponentCompilationService::new(&config);
        let outcomes = ["accepted", "rejected", "failed", "timeout"];
        let counters = outcomes
            .map(|label| crate::metrics::COMPILATION_ENQUEUE_TOTAL.with_label_values(&[label]));
        let before = counters.each_ref().map(|counter| counter.get());
        tokio::time::timeout(
            Duration::from_millis(900),
            stalled_service.enqueue_compilation(
                EnvironmentId::new(),
                ComponentId::new(),
                ComponentRevision::INITIAL,
            ),
        )
        .await
        .expect("a silent peer must fit the enqueue budget");
        for ((label, counter), before) in outcomes.into_iter().zip(counters).zip(before) {
            assert_eq!(counter.get(), before + u64::from(label == "timeout"));
        }
    }
}
