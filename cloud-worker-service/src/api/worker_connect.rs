use std::sync::Arc;
use std::time::Duration;

use crate::service::auth::CloudAuthCtx;
use crate::service::worker::{ConnectWorkerStream, WorkerService};
use cloud_common::auth::WrappedGolemSecuritySchema;
use cloud_common::model::TokenSecret;
use futures_util::StreamExt;
use golem_common::model::ComponentId;
use golem_common::recorded_http_api_request;
use golem_service_base::model::WorkerId;
use golem_worker_service_base::service::worker::proxy_worker_connection;
use http::StatusCode;
use poem::web::websocket::WebSocket;
use poem::web::{Data, Path};
use poem::*;
use tracing::Instrument;

#[derive(Clone)]
pub struct ConnectService {
    worker_service: Arc<dyn WorkerService + Send + Sync>,
}

impl ConnectService {
    pub fn new(worker_service: Arc<dyn WorkerService + Send + Sync>) -> Self {
        Self { worker_service }
    }
}

#[handler]
pub async fn ws(
    Path((component_id, worker_name)): Path<(ComponentId, String)>,
    websocket: WebSocket,
    Data(service): Data<&ConnectService>,
    token: WrappedGolemSecuritySchema,
) -> Response {
    tracing::info!("Connect worker {}/{}", component_id, worker_name);
    get_worker_stream(service, component_id, worker_name, token.0.secret())
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
                            PING_INTERVAL,
                            PING_TIMEOUT,
                        )
                        .await;
                    })
                })
                .into_response()
        })
        .unwrap_or_else(|err| err)
}

async fn get_worker_stream(
    service: &ConnectService,
    component_id: ComponentId,
    worker_name: String,
    token: TokenSecret,
) -> Result<(WorkerId, ConnectWorkerStream), Response> {
    let worker_id = WorkerId::new(component_id, worker_name)
        .map_err(|e| single_error(StatusCode::BAD_REQUEST, format!("Invalid worker id: {e}")))?;

    let record = recorded_http_api_request!("connect_worker", worker_id = worker_id.to_string());

    let result = service
        .worker_service
        .connect(&worker_id, &CloudAuthCtx::new(token))
        .instrument(record.span.clone())
        .await;

    match result {
        Ok(worker_stream) => record.succeed(Ok((worker_id, worker_stream))),
        Err(error) => {
            tracing::info!("Error connecting to worker {error}");
            let error = record.fail(error, &("InternalError")); // TODO: Use a better error tags instead of InternalError
            Err(error.into_response())
        }
    }
}

const PING_INTERVAL: Duration = Duration::from_secs(30);
const PING_TIMEOUT: Duration = Duration::from_secs(15);

fn single_error(status: StatusCode, message: impl std::fmt::Display) -> Response {
    let body = golem_service_base::model::ErrorBody {
        error: message.to_string(),
    };

    let body = serde_json::to_string(&body).expect("Serialize ErrorBody");

    Response::builder()
        .status(status)
        .content_type("application/json")
        .body(body)
}

fn many_errors(status: StatusCode, errors: Vec<String>) -> Response {
    let body = golem_service_base::model::ErrorsBody { errors };

    let body = serde_json::to_string(&body).expect("Serialize ErrorsBody");

    Response::builder()
        .status(status)
        .content_type("application/json")
        .body(body)
}

impl IntoResponse for crate::service::worker::WorkerError {
    fn into_response(self) -> Response {
        use golem_worker_service_base::service::component::ComponentServiceError;
        use golem_worker_service_base::service::worker::WorkerServiceError;

        match self {
            crate::service::worker::WorkerError::Base(error) => match error {
                WorkerServiceError::Component(error) => match error {
                    ComponentServiceError::BadRequest(errors) => {
                        many_errors(StatusCode::BAD_REQUEST, errors)
                    }
                    ComponentServiceError::AlreadyExists(_) => {
                        single_error(StatusCode::CONFLICT, error)
                    }
                    ComponentServiceError::Internal(_) => {
                        single_error(StatusCode::INTERNAL_SERVER_ERROR, error)
                    }
                    ComponentServiceError::Unauthorized(error) => {
                        single_error(StatusCode::UNAUTHORIZED, error)
                    }
                    ComponentServiceError::Forbidden(error) => {
                        single_error(StatusCode::FORBIDDEN, error)
                    }
                    ComponentServiceError::NotFound(error) => {
                        single_error(StatusCode::NOT_FOUND, error)
                    }
                },
                WorkerServiceError::TypeChecker(_) => single_error(StatusCode::BAD_REQUEST, error),
                WorkerServiceError::VersionedComponentIdNotFound(_)
                | WorkerServiceError::ComponentNotFound(_)
                | WorkerServiceError::AccountIdNotFound(_)
                | WorkerServiceError::WorkerNotFound(_) => {
                    single_error(StatusCode::NOT_FOUND, error)
                }
                WorkerServiceError::Golem(_) | WorkerServiceError::Internal(_) => {
                    single_error(StatusCode::INTERNAL_SERVER_ERROR, error)
                }
            },
            crate::service::worker::WorkerError::Forbidden(_) => {
                single_error(StatusCode::FORBIDDEN, self)
            }
            crate::service::worker::WorkerError::Unauthorized(_) => {
                single_error(StatusCode::UNAUTHORIZED, self)
            }
            crate::service::worker::WorkerError::ProjectNotFound(_) => {
                single_error(StatusCode::NOT_FOUND, self)
            }
            crate::service::worker::WorkerError::Internal(_) => {
                single_error(StatusCode::INTERNAL_SERVER_ERROR, self)
            }
        }
    }
}
