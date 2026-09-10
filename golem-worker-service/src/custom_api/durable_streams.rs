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

use super::{ResponseBody, RouteExecutionResult};
use http::StatusCode;
use std::collections::HashMap;

pub(super) fn handle_request() -> RouteExecutionResult {
    RouteExecutionResult {
        status: StatusCode::NOT_IMPLEMENTED,
        headers: HashMap::new(),
        body: ResponseBody::NoBody,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn durable_stream_handler_returns_not_implemented() {
        let response = handle_request();
        assert_eq!(response.status, StatusCode::NOT_IMPLEMENTED);
        assert!(matches!(response.body, ResponseBody::NoBody));
    }
}
