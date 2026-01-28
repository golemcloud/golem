// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::api::common::ApiEndpointError;
use crate::custom_api::request_handler::RequestHandler;
use futures::{FutureExt, TryFutureExt};
use golem_common::recorded_http_api_request;
use poem::{Endpoint, IntoResponse, Request, Response};
use std::future::Future;
use std::sync::Arc;
use tracing::Instrument;

pub struct CustomApiPoemEndpoint {
    pub request_handler: Arc<RequestHandler>,
}

impl CustomApiPoemEndpoint {
    pub fn new(request_handler: Arc<RequestHandler>) -> Self {
        Self { request_handler }
    }

    pub async fn execute(&self, request: Request) -> Response {
        let record = recorded_http_api_request!("execute",
            method = %request.method(),
            uri = %request.uri()
        );

        let response = self
            .request_handler
            .handle_request(request)
            .instrument(record.span.clone())
            .map_err(ApiEndpointError::from)
            .await;

        record
            .result(response)
            .unwrap_or_else(IntoResponse::into_response)
    }
}

impl Endpoint for CustomApiPoemEndpoint {
    type Output = Response;

    fn call(&self, req: Request) -> impl Future<Output = poem::Result<Self::Output>> + Send {
        self.execute(req).map(Ok)
    }
}
