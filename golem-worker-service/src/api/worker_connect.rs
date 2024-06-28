// Copyright 2024 Golem Cloud
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

use futures::StreamExt;
use golem_common::model::ComponentId;
use golem_service_base::model::WorkerId;
use golem_worker_service_base::auth::EmptyAuthCtx;
use golem_worker_service_base::service::worker::{proxy_worker_connection, ConnectWorkerStream};
use poem::web::websocket::WebSocket;
use poem::web::Data;
use poem::*;

use crate::empty_worker_metadata;
use crate::service::worker::WorkerService;

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
    req: &Request,
    websocket: WebSocket,
    Data(service): Data<&ConnectService>,
) -> Response {
    tracing::info!("Connect request: {:?} {:?}", req.uri(), req);

    get_worker_stream(service, req)
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

async fn get_worker_stream(
    service: &ConnectService,
    req: &Request,
) -> Result<(WorkerId, ConnectWorkerStream), Response> {
    let worker_id = match get_worker_id(req) {
        Ok(worker_id) => worker_id,
        Err(err) => return Err((http::StatusCode::BAD_REQUEST, err).into_response()),
    };

    let worker_stream = service
        .worker_service
        .connect(
            &worker_id,
            empty_worker_metadata(),
            &EmptyAuthCtx::default(),
        )
        .await
        .map_err(|e| (http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok((worker_id, worker_stream))
}

fn get_worker_id(req: &Request) -> Result<WorkerId, String> {
    let (component_id, worker_name) = req.path_params::<(String, String)>().map_err(|_| {
        "Valid path parameters (component_id and worker_name) are required ".to_string()
    })?;

    let component_id = ComponentId::try_from(component_id.as_str())
        .map_err(|error| format!("Invalid component id: {error}"))?;

    let worker_id = WorkerId::new(component_id, worker_name)
        .map_err(|error| format!("Invalid worker name: {error}"))?;

    Ok(worker_id)
}
