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

pub use crate::base_model::auth::*;
use base64::Engine;
use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
use poem_openapi::types::{ParseError, ParseFromJSON, ParseResult, ToJSON, Type};
use rand::TryRngCore;
use rand::rngs::OsRng;
use std::borrow::Cow;
use std::str::FromStr;

impl Default for TokenSecret {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenSecret {
    pub fn new() -> Self {
        let mut token = [0u8; 32]; // 32 bytes = 256 bits
        OsRng
            .try_fill_bytes(&mut token)
            .expect("Failed to generate random bytes");
        let token_str = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token);
        Self(token_str)
    }
}

impl Type for TokenSecret {
    const IS_REQUIRED: bool = true;

    type RawValueType = Self;

    type RawElementValueType = Self;

    fn name() -> Cow<'static, str> {
        "string".into()
    }

    fn schema_ref() -> MetaSchemaRef {
        let mut meta = MetaSchema::new("string");
        meta.min_length = Some(16);
        meta.pattern = Some("^[A-Za-z0-9_-]+$".to_string());

        MetaSchemaRef::Inline(Box::new(meta))
    }

    fn as_raw_value(&self) -> Option<&Self::RawValueType> {
        Some(self)
    }

    fn raw_element_iter<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a Self::RawElementValueType> + 'a> {
        Box::new(self.as_raw_value().into_iter())
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl ParseFromJSON for TokenSecret {
    fn parse_from_json(value: Option<serde_json::Value>) -> ParseResult<Self> {
        let value = value.unwrap_or_default();
        if let serde_json::Value::String(v) = value {
            TokenSecret::from_str(&v).map_err(ParseError::custom)
        } else {
            Err(ParseError::expected_type(value))
        }
    }
}

impl ToJSON for TokenSecret {
    fn to_json(&self) -> Option<serde_json::Value> {
        Some(serde_json::Value::String(self.0.clone()))
    }
}

mod protobuf {
    use super::{AccountRole, EnvironmentRole};

    impl TryFrom<golem_api_grpc::proto::golem::auth::AccountRole> for AccountRole {
        type Error = String;
        fn try_from(
            value: golem_api_grpc::proto::golem::auth::AccountRole,
        ) -> Result<Self, Self::Error> {
            use golem_api_grpc::proto::golem::auth::AccountRole as GrpcAccountRole;
            match value {
                GrpcAccountRole::Admin => Ok(Self::Admin),
                GrpcAccountRole::MarketingAdmin => Ok(Self::MarketingAdmin),
                GrpcAccountRole::Unspecified => Err("unknown account role".to_string()),
            }
        }
    }

    impl From<AccountRole> for golem_api_grpc::proto::golem::auth::AccountRole {
        fn from(value: AccountRole) -> Self {
            match value {
                AccountRole::Admin => Self::Admin,
                AccountRole::MarketingAdmin => Self::MarketingAdmin,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::auth::EnvironmentRole> for EnvironmentRole {
        type Error = String;
        fn try_from(
            value: golem_api_grpc::proto::golem::auth::EnvironmentRole,
        ) -> Result<Self, Self::Error> {
            use golem_api_grpc::proto::golem::auth::EnvironmentRole as GrpcEnvironmentRole;

            match value {
                GrpcEnvironmentRole::Admin => Ok(Self::Admin),
                GrpcEnvironmentRole::Viewer => Ok(Self::Viewer),
                GrpcEnvironmentRole::Deployer => Ok(Self::Deployer),
                GrpcEnvironmentRole::Unspecified => Err("unknown environment role".to_string()),
            }
        }
    }

    impl From<EnvironmentRole> for golem_api_grpc::proto::golem::auth::EnvironmentRole {
        fn from(value: EnvironmentRole) -> Self {
            match value {
                EnvironmentRole::Admin => Self::Admin,
                EnvironmentRole::Viewer => Self::Viewer,
                EnvironmentRole::Deployer => Self::Deployer,
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::TokenSecret;
    use test_r::test;

    #[test]
    #[ignore]
    // utility to generate a token secret for tests, configs, etc.
    fn generate_token_secret() {
        let secret = TokenSecret::new();
        println!("Token secret: {}", secret.0)
    }
}
