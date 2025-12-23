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

use std::future::Future;
use std::sync::Arc;

use crate::gateway_execution::gateway_http_input_executor::GatewayHttpInputExecutor;
use futures::FutureExt;
use poem::{Endpoint, Request, Response};

pub struct CustomHttpRequestApi {
    pub gateway_http_input_executor: Arc<GatewayHttpInputExecutor>,
}

impl CustomHttpRequestApi {
    pub fn new(gateway_http_input_executor: Arc<GatewayHttpInputExecutor>) -> Self {
        Self {
            gateway_http_input_executor,
        }
    }

    pub async fn execute(&self, request: Request) -> Response {
        self.gateway_http_input_executor
            .execute_http_request(request)
            .await
    }
}

impl Endpoint for CustomHttpRequestApi {
    type Output = Response;

    fn call(&self, req: Request) -> impl Future<Output = poem::Result<Self::Output>> + Send {
        self.execute(req).map(Ok)
    }
}
