// Copyright 2024-2026 Golem Cloud
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

//! Validation of the principal in a 1.6 snapshot envelope.
//!
//! Snapshots written by 1.6 SDKs carry the 1.6 principal. A 2.0 agent reads such a snapshot with
//! the same strictness as before (a malformed principal still fails the load) and then drops the
//! principal: the caller of an invocation comes from the host, never from saved state.

use serde::Deserialize;
use serde::de;

fn uuid_text(text: &str) -> Result<(), uuid::Error> {
    uuid::Uuid::parse_str(text).map(|_| ())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct OidcPrincipal {
    sub: String,
    issuer: String,
    email: Option<String>,
    name: Option<String>,
    email_verified: Option<bool>,
    given_name: Option<String>,
    family_name: Option<String>,
    picture: Option<String>,
    preferred_username: Option<String>,
    claims: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct AgentPrincipal {
    component_id: String,
    agent_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct GolemUserPrincipal {
    account_id: String,
}

/// Checks that `bytes` is a well-formed 1.6 principal (`{"tag":..,"val":..}`), and discards it.
pub(super) fn validate_json_bytes(bytes: &[u8]) -> Result<(), serde_json::Error> {
    #[derive(Deserialize)]
    struct Envelope<'a> {
        tag: String,
        #[serde(borrow)]
        val: Option<&'a serde_json::value::RawValue>,
    }

    let envelope: Envelope<'_> = serde_json::from_slice(bytes)?;
    if envelope.tag == "anonymous" {
        return Ok(());
    }
    let value = envelope
        .val
        .ok_or_else(|| de::Error::missing_field("val"))?;
    match envelope.tag.as_str() {
        "oidc" => serde_json::from_str::<OidcPrincipal>(value.get()).map(|_| ()),
        "agent" => {
            let agent: AgentPrincipal = serde_json::from_str(value.get())?;
            uuid_text(&agent.component_id).map_err(de::Error::custom)
        }
        "golem-user" => {
            let user: GolemUserPrincipal = serde_json::from_str(value.get())?;
            uuid_text(&user.account_id).map_err(de::Error::custom)
        }
        other => Err(de::Error::unknown_variant(
            other,
            &["anonymous", "oidc", "agent", "golem-user"],
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::validate_json_bytes;
    use test_r::test;

    #[test]
    fn principal_payloads_validate_in_either_field_order() {
        for json in [
            r#"{"tag":"anonymous"}"#,
            r#"{"val":{"componentId":"10203040-5060-7080-9012-3456789abcde","agentId":"worker(7)"},"tag":"agent"}"#,
            r#"{"tag":"golem-user","val":{"accountId":"fedcba98-7654-3210-9876-543210abcdef"}}"#,
            r#"{"val":{"sub":"subject","issuer":"https://issuer","email":"a@example.test","emailVerified":false,"name":null,"givenName":null,"familyName":"Family","picture":null,"preferredUsername":"login","claims":"{\"role\":\"reader\"}"},"tag":"oidc"}"#,
            r#"{"val":{"ignored":true},"extra":7,"tag":"anonymous"}"#,
        ] {
            validate_json_bytes(json.as_bytes()).unwrap_or_else(|error| panic!("{json}: {error}"));
        }
    }

    #[test]
    fn principal_payloads_reject_missing_or_invalid_typed_fields() {
        for json in [
            r#"{"val":{}}"#,
            r#"{"tag":"missing"}"#,
            r#"{"tag":"agent"}"#,
            r#"{"tag":"oidc","val":null}"#,
            r#"{"tag":"golem-user","val":{"accountId":"not-a-uuid"}}"#,
            r#"{"tag":"agent","val":{"componentId":"10203040-5060-7080-9012-3456789abcde","agentId":4}}"#,
        ] {
            assert!(validate_json_bytes(json.as_bytes()).is_err(), "{json}");
        }
    }
}
