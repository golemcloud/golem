use std::borrow::Cow;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

use bincode::de::read::Reader;
use bincode::de::{BorrowDecoder, Decoder};
use bincode::enc::write::Writer;
use bincode::enc::Encoder;
use bincode::error::{DecodeError, EncodeError};
use bincode::{BorrowDecode, Decode, Encode};
use derive_more::FromStr;
use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
use poem_openapi::types::{ParseFromJSON, ParseFromParameter, ParseResult, ToJSON};
use poem_openapi::Object;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use golem_common::newtype_uuid;

newtype_uuid!(PlanId, cloud_api_grpc::proto::golem::cloud::plan::PlanId);
newtype_uuid!(
    ProjectGrantId,
    cloud_api_grpc::proto::golem::cloud::projectgrant::ProjectGrantId
);
newtype_uuid!(
    ProjectPolicyId,
    cloud_api_grpc::proto::golem::cloud::projectpolicy::ProjectPolicyId
);
newtype_uuid!(TokenId, cloud_api_grpc::proto::golem::cloud::token::TokenId);

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
pub struct TokenSecret {
    pub value: Uuid,
}

impl TokenSecret {
    pub fn new(value: Uuid) -> Self {
        Self { value }
    }
}

impl TryFrom<cloud_api_grpc::proto::golem::cloud::token::TokenSecret> for TokenSecret {
    type Error = String;

    fn try_from(
        value: cloud_api_grpc::proto::golem::cloud::token::TokenSecret,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            value: value.value.ok_or("Missing field: value")?.into(),
        })
    }
}

impl From<TokenSecret> for cloud_api_grpc::proto::golem::cloud::token::TokenSecret {
    fn from(value: TokenSecret) -> Self {
        Self {
            value: Some(value.value.into()),
        }
    }
}

impl std::str::FromStr for TokenSecret {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let uuid = Uuid::parse_str(s).map_err(|err| format!("Invalid token: {err}"))?;
        Ok(Self { value: uuid })
    }
}
