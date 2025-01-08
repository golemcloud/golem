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

use std::time::Duration;

use crate::service::worker::WorkerService;
use futures::StreamExt;
use golem_api_grpc::proto::golem::worker::LogEvent;
use golem_common::model::{ComponentId, WorkerId};
use golem_common::recorded_http_api_request;
use golem_service_base::model::{validate_worker_name, ErrorsBody};
use golem_worker_service_base::api::WorkerApiBaseError;
use golem_worker_service_base::empty_worker_metadata;
use golem_worker_service_base::service::worker::{proxy_worker_connection, WorkerStream};
use poem::web::websocket::WebSocket;
use poem::web::{Data, Path};
use poem::*;
use poem_openapi::payload::Json;
use tracing::Instrument;

#[derive(Clone)]
pub struct ConnectService {
    worker_service: WorkerService,
}

impl ConnectService {
    pub fn new(worker_service: WorkerService) -> Self {
        Self { worker_service }
    }
}

#[handler]
pub async fn ws(
    Path((component_id, worker_name)): Path<(ComponentId, String)>,
    websocket: WebSocket,
    Data(service): Data<&ConnectService>,
) -> Response {
    connect_to_worker(service, component_id, worker_name)
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

const PING_INTERVAL: Duration = Duration::from_secs(30);
const PING_TIMEOUT: Duration = Duration::from_secs(15);

async fn connect_to_worker(
    service: &ConnectService,
    component_id: ComponentId,
    worker_name: String,
) -> Result<(WorkerId, WorkerStream<LogEvent>), Response> {
    validate_worker_name(&worker_name).map_err(|e| {
        let error = WorkerApiBaseError::BadRequest(Json(ErrorsBody {
            errors: vec![format!("Invalid worker name: {e}")],
        }));
        error.into_response()
    })?;
    let worker_id = WorkerId {
        component_id: component_id.clone(),
        worker_name: worker_name.clone(),
    };

    let record = recorded_http_api_request!("connect_worker", worker_id = worker_id.to_string());

    let result = service
        .worker_service
        .connect(&worker_id, empty_worker_metadata())
        .instrument(record.span.clone())
        .await;

    match result {
        Ok(worker_stream) => record.succeed(Ok((worker_id, worker_stream))),
        Err(error) => {
            tracing::error!("Error connecting to worker: {error}");
            let error = WorkerApiBaseError::from(error);
            let error = record.fail(error.clone(), &error);
            Err(error.into_response())
        }
    }
}
