use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use golem_common::model::TemplateId;
use golem_service_base::model::WorkerId;
use golem_worker_service_base::service::worker::proxy_worker_connection;
use http::StatusCode;
use poem::web::websocket::WebSocket;
use poem::web::{Data, Path};
use poem::*;
use tap::TapFallible;

use crate::api::auth::AuthData;
use crate::auth::AccountAuthorisation;
use crate::service::worker::{ConnectWorkerStream, WorkerService};

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
    Path((template_id, worker_name)): Path<(TemplateId, String)>,
    websocket: WebSocket,
    Data(service): Data<&ConnectService>,
    auth: AuthData,
) -> Response {
    get_worker_stream(service, template_id, worker_name, auth.auth)
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
    template_id: TemplateId,
    worker_name: String,
    auth: AccountAuthorisation,
) -> Result<(WorkerId, ConnectWorkerStream), Response> {
    let worker_id = WorkerId::new(template_id, worker_name)
        .map_err(|e| single_error(StatusCode::BAD_REQUEST, format!("Invalid worker id: {e}")))?;

    let worker_stream = service
        .worker_service
        .connect(&worker_id, &auth)
        .await
        .tap_err(|e| tracing::info!("Error connecting to worker {e}"))
        .map_err(|e| e.into_response())?;

    Ok((worker_id, worker_stream))
}

const PING_INTERVAL: Duration = Duration::from_secs(30);
const PING_TIMEOUT: Duration = Duration::from_secs(15);

fn single_error(status: StatusCode, message: impl std::fmt::Display) -> Response {
    let body = golem_service_base::model::ErrorBody {
        error: message.to_string(),
    };

    let body = serde_json::to_string(&body).expect("Serialize ErrorBody");

    poem::Response::builder()
        .status(status)
        .content_type("application/json")
        .body(body)
}

fn many_errors(status: StatusCode, errors: Vec<String>) -> Response {
    let body = golem_service_base::model::ErrorsBody { errors };

    let body = serde_json::to_string(&body).expect("Serialize ErrorsBody");

    poem::Response::builder()
        .status(status)
        .content_type("application/json")
        .body(body)
}

impl IntoResponse for crate::service::worker::WorkerError {
    fn into_response(self) -> Response {
        use golem_worker_service_base::service::template::TemplateServiceError;
        use golem_worker_service_base::service::worker::WorkerServiceError;

        match self {
            crate::service::worker::WorkerError::Base(error) => match error {
                WorkerServiceError::Template(error) => match error {
                    TemplateServiceError::BadRequest(errors) => {
                        many_errors(StatusCode::BAD_REQUEST, errors)
                    }
                    TemplateServiceError::AlreadyExists(_) => {
                        single_error(StatusCode::CONFLICT, error)
                    }
                    TemplateServiceError::Internal(_) => {
                        single_error(StatusCode::INTERNAL_SERVER_ERROR, error)
                    }
                    TemplateServiceError::Unauthorized(error) => {
                        single_error(StatusCode::UNAUTHORIZED, error)
                    }
                    TemplateServiceError::Forbidden(error) => {
                        single_error(StatusCode::FORBIDDEN, error)
                    }
                    TemplateServiceError::NotFound(error) => {
                        single_error(StatusCode::NOT_FOUND, error)
                    }
                },
                WorkerServiceError::TypeChecker(_) => single_error(StatusCode::BAD_REQUEST, error),
                WorkerServiceError::VersionedTemplateIdNotFound(_)
                | WorkerServiceError::TemplateNotFound(_)
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
        }
    }
}
