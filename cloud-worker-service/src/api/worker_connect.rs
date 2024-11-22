use crate::api::worker::WorkerError;
use crate::service::auth::AuthService;
use crate::service::worker::{ConnectWorkerStream, WorkerService};
use cloud_common::auth::{CloudAuthCtx, WrappedGolemSecuritySchema};
use cloud_common::model::{ProjectAction, TokenSecret};
use futures_util::StreamExt;
use golem_common::model::{ComponentId, WorkerId};
use golem_common::recorded_http_api_request;
use golem_service_base::model::{validate_worker_name, ErrorBody, ErrorsBody};
use golem_worker_service_base::service::worker::proxy_worker_connection;
use poem::web::websocket::WebSocket;
use poem::web::{Data, Path};
use poem::*;
use poem_openapi::payload::Json;
use std::sync::Arc;
use std::time::Duration;
use tracing::Instrument;

#[derive(Clone)]
pub struct ConnectService {
    worker_service: Arc<dyn WorkerService + Send + Sync>,
    auth_service: Arc<dyn AuthService + Send + Sync>,
}

impl ConnectService {
    pub fn new(
        worker_service: Arc<dyn WorkerService + Send + Sync>,
        auth_service: Arc<dyn AuthService + Send + Sync>,
    ) -> Self {
        Self {
            worker_service,
            auth_service,
        }
    }

    async fn get_worker_stream(
        &self,
        component_id: ComponentId,
        worker_name: String,
        token: TokenSecret,
    ) -> Result<(WorkerId, ConnectWorkerStream), Response> {
        validate_worker_name(&worker_name).map_err(|e| {
            let error = WorkerError::BadRequest(Json(ErrorsBody {
                errors: vec![format!("Invalid worker id: {e}")],
            }));
            error.into_response()
        })?;

        let worker_id = WorkerId {
            component_id: component_id.clone(),
            worker_name: worker_name.clone(),
        };

        let record =
            recorded_http_api_request!("connect_worker", worker_id = worker_id.to_string());

        let auth = CloudAuthCtx::new(token);
        let namespace = self
            .auth_service
            .is_authorized_by_component(&component_id, ProjectAction::ViewWorker, &auth)
            .await
            .map_err(|e| {
                let error = WorkerError::Unauthorized(Json(ErrorBody {
                    error: format!("Unauthorized: {e}"),
                }));
                error.into_response()
            })?;
        let result = self
            .worker_service
            .connect(&worker_id, namespace)
            .instrument(record.span.clone())
            .await;

        match result {
            Ok(worker_stream) => record.succeed(Ok((worker_id, worker_stream))),
            Err(error) => {
                tracing::error!("Error connecting to worker: {error}");
                let error = WorkerError::from(error);
                let error = record.fail(error.clone(), &error);
                Err(error.into_response())
            }
        }
    }

    const PING_INTERVAL: Duration = Duration::from_secs(30);
    const PING_TIMEOUT: Duration = Duration::from_secs(15);
}

#[handler]
pub async fn ws(
    Path((component_id, worker_name)): Path<(ComponentId, String)>,
    websocket: WebSocket,
    Data(service): Data<&ConnectService>,
    token: WrappedGolemSecuritySchema,
) -> Response {
    tracing::info!("Connect worker {}/{}", component_id, worker_name);
    service
        .get_worker_stream(component_id, worker_name, token.0.secret())
        .await
        .map(|(worker_id, worker_stream)| {
            websocket
                .on_upgrade(move |socket| {
                    tokio::spawn(async move {
                        let (sink, stream) = socket.split();
                        let _ = proxy_worker_connection(
                            worker_id,
                            worker_stream,
                            sink,
                            stream,
                            ConnectService::PING_INTERVAL,
                            ConnectService::PING_TIMEOUT,
                        )
                        .await;
                    })
                })
                .into_response()
        })
        .unwrap_or_else(|err| err)
}
