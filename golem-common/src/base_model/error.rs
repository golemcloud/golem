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

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
pub struct ErrorsBody {
    pub code: String,
    pub errors: Vec<String>,

    #[cfg_attr(feature = "full", oai(skip))]
    #[serde(skip)]
    pub cause: Option<anyhow::Error>,
}

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
pub struct ErrorBody {
    pub code: String,
    pub error: String,

    #[cfg_attr(feature = "full", oai(skip))]
    #[serde(skip)]
    pub cause: Option<anyhow::Error>,
}

/// The body of a `403`. It names the missing grant only when the authorizer gives one, which it
/// does only for a target that the caller may view (5.3 Rule 21).
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ForbiddenErrorBody {
    pub code: String,
    pub error: String,
    /// The grant that would allow the request, in its text form.
    #[cfg_attr(feature = "full", oai(skip_serializing_if_is_none))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing_grant: Option<String>,

    #[cfg_attr(feature = "full", oai(skip))]
    #[serde(skip)]
    pub cause: Option<anyhow::Error>,
}

#[cfg(test)]
mod tests {
    use super::ForbiddenErrorBody;
    use test_r::test;

    #[test]
    fn a_forbidden_body_carries_the_missing_grant_only_when_set() {
        let without: ForbiddenErrorBody =
            serde_json::from_str(r#"{"code":"AUTH_FORBIDDEN","error":"Access denied"}"#).unwrap();
        assert_eq!(without.missing_grant, None);
        assert_eq!(
            serde_json::to_value(&without).unwrap(),
            serde_json::json!({"code": "AUTH_FORBIDDEN", "error": "Access denied"})
        );

        let with: ForbiddenErrorBody = serde_json::from_str(
            r#"{"code":"AUTH_FORBIDDEN","error":"Access denied","missingGrant":"tool(E/grep) : invoke"}"#,
        )
        .unwrap();
        assert_eq!(with.missing_grant.as_deref(), Some("tool(E/grep) : invoke"));
        assert_eq!(
            serde_json::to_value(&with).unwrap()["missingGrant"],
            "tool(E/grep) : invoke"
        );
    }
}
