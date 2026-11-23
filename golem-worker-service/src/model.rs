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

use golem_common::model::agent::AgentMetadataDto;
use golem_common::model::{PageScanGrant, ScanCursor};
use poem_openapi::Object;
use std::fmt::Debug;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct WorkersMetadataResponse {
    pub workers: Vec<AgentMetadataDto>,
    pub cursor: Option<ScanCursor>,
    // Present only for a call of a listing page: what the executor granted the call.
    pub page_scan: Option<PageScanGrant>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use poem_openapi::types::ToJSON;
    use test_r::test;

    #[test]
    fn a_page_call_response_names_its_grant_page_scan() {
        let response = WorkersMetadataResponse {
            workers: Vec::new(),
            cursor: None,
            page_scan: Some(PageScanGrant {
                calls: 10,
                keys: 10_000,
                millis: 5_000,
            }),
        };
        let grant = serde_json::json!({"calls": 10, "keys": 10_000, "millis": 5_000});
        assert_eq!(response.to_json().unwrap()["pageScan"], grant);
        assert_eq!(serde_json::to_value(&response).unwrap()["pageScan"], grant);
    }
}
