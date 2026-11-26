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

use aws_credential_types::Credentials;
use http::HeaderValue;
use std::fmt::{Debug, Formatter};

/// Static host-side credentials. This type has no serialization implementation.
#[derive(Clone)]
pub struct AwsCredentials(pub(crate) Credentials);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CredentialsError {
    #[error("invalid awsSigV4 field: {0}")]
    InvalidField(&'static str),
}

impl AwsCredentials {
    /// Reads an `awsSigV4` record. An error names the field, not its value.
    pub fn from_record(record: &serde_json::Value) -> Result<Self, CredentialsError> {
        let field = |name| {
            record
                .get(name)
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.is_empty() && HeaderValue::from_str(value).is_ok())
                .ok_or(CredentialsError::InvalidField(name))
        };
        let access_key = field("accessKeyId")?;
        let secret_key = field("secretAccessKey")?;
        let token = record
            .get("sessionToken")
            .filter(|value| !value.is_null())
            .map(|_| field("sessionToken"))
            .transpose()?;
        Ok(Self(Credentials::new(
            access_key,
            secret_key,
            token.map(str::to_owned),
            None,
            "volume",
        )))
    }
}

impl Debug for AwsCredentials {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("AwsCredentials([redacted])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn credentials_validate_fields_and_keep_values_out_of_diagnostics() {
        let record = serde_json::json!({
            "accessKeyId": "hidden-access",
            "secretAccessKey": "hidden-secret",
            "sessionToken": "hidden-token"
        });
        let credentials = AwsCredentials::from_record(&record).unwrap();
        assert_eq!(format!("{credentials:?}"), "AwsCredentials([redacted])");
        ["accessKeyId", "secretAccessKey", "sessionToken"]
            .into_iter()
            .for_each(|field| {
                [
                    serde_json::json!(17),
                    serde_json::json!(""),
                    serde_json::json!("hidden-secret\r\nhidden-token"),
                    serde_json::json!("hidden-secret\0hidden-token"),
                ]
                .into_iter()
                .for_each(|value| {
                    let mut invalid = record.clone();
                    invalid[field] = value;
                    let error = AwsCredentials::from_record(&invalid).unwrap_err();
                    assert_eq!(error, CredentialsError::InvalidField(field));
                    let text = format!("{error:?} {error}");
                    assert!(!text.contains("hidden-secret"));
                    assert!(!text.contains("hidden-token"));
                });
            });
        ["accessKeyId", "secretAccessKey"]
            .into_iter()
            .for_each(|field| {
                let mut invalid = record.clone();
                invalid.as_object_mut().unwrap().remove(field);
                assert_eq!(
                    AwsCredentials::from_record(&invalid).unwrap_err(),
                    CredentialsError::InvalidField(field)
                );
            });
        [
            serde_json::json!({"accessKeyId": "access", "secretAccessKey": "secret"}),
            serde_json::json!({"accessKeyId": "access", "secretAccessKey": "secret", "sessionToken": null}),
        ].iter().for_each(|record| assert!(AwsCredentials::from_record(record).is_ok()));
    }
}
