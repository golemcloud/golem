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

use crate::base_model::auth::AccountRole;
use crate::base_model::card::CardId;
use crate::base_model::plan::PlanId;
use crate::schema::conversion::{FromSchemaError, SchemaBuilder, default_type_id_from};
use crate::schema::metadata::{MetadataEnvelope, TypeId};
use crate::schema::schema_type::{NamedFieldType, SchemaType};
use crate::schema::schema_value::SchemaValue;
use crate::{declare_revision, declare_structs, newtype_uuid};
use derive_more::Display;
use serde::{Deserialize, Serialize};
use uuid::uuid;

newtype_uuid!(AccountId, wit_name: "account-id", wit_owner: "golem:core@2.0.0/types", golem_api_grpc::proto::golem::common::AccountId);

impl AccountId {
    pub const SYSTEM: Self = AccountId(uuid!("00000000-0000-0000-0000-000000000000"));
}

impl crate::schema::conversion::IntoSchema for AccountId {
    fn type_id() -> TypeId {
        default_type_id_from(std::any::type_name::<Self>())
    }

    fn register_in(builder: &mut SchemaBuilder) -> SchemaType {
        let id = <Self as crate::schema::conversion::IntoSchema>::type_id();
        if builder.is_registered(&id) {
            return SchemaType::ref_to(id);
        }

        builder.reserve(id.clone());
        let body = SchemaType::record(vec![NamedFieldType {
            name: "uuid".to_string(),
            body: <uuid::Uuid as crate::schema::conversion::IntoSchema>::register_in(builder),
            metadata: MetadataEnvelope::default(),
        }]);
        builder.commit(
            id.clone(),
            Some("account-id".to_string()),
            MetadataEnvelope::default(),
            body,
        );
        SchemaType::ref_to(id)
    }

    fn to_value(&self) -> SchemaValue {
        SchemaValue::Record {
            fields: vec![<uuid::Uuid as crate::schema::conversion::IntoSchema>::to_value(&self.0)],
        }
    }
}

impl crate::schema::conversion::FromSchema for AccountId {
    fn from_value(value: &SchemaValue) -> Result<Self, FromSchemaError> {
        match value {
            SchemaValue::Record { fields } if fields.len() == 1 => Ok(Self(
                <uuid::Uuid as crate::schema::conversion::FromSchema>::from_value(&fields[0])?,
            )),
            other => Err(FromSchemaError::shape_mismatch(
                "record with one uuid field",
                format!("{other:?}"),
                "AccountId",
            )),
        }
    }
}

declare_revision!(AccountRevision);

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Display)]
#[cfg_attr(feature = "full", derive(poem_openapi::NewType))]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
#[cfg_attr(
    feature = "full",
    oai(from_json = false, from_parameter = false, from_multipart = false)
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct AccountEmail(String);

impl AccountEmail {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into().to_ascii_lowercase())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl crate::schema::conversion::IntoSchema for AccountEmail {
    fn type_id() -> TypeId {
        default_type_id_from(std::any::type_name::<Self>())
    }

    fn register_in(builder: &mut SchemaBuilder) -> SchemaType {
        let id = <Self as crate::schema::conversion::IntoSchema>::type_id();
        if builder.is_registered(&id) {
            return SchemaType::ref_to(id);
        }

        builder.reserve(id.clone());
        builder.commit(
            id.clone(),
            Some("account-email".to_string()),
            MetadataEnvelope::default(),
            SchemaType::string(),
        );
        SchemaType::ref_to(id)
    }

    fn to_value(&self) -> SchemaValue {
        SchemaValue::String(self.0.clone())
    }
}

impl crate::schema::conversion::FromSchema for AccountEmail {
    fn from_value(value: &SchemaValue) -> Result<Self, FromSchemaError> {
        match value {
            SchemaValue::String(value) => Ok(Self::new(value.clone())),
            other => Err(FromSchemaError::shape_mismatch(
                "string",
                format!("{other:?}"),
                "AccountEmail",
            )),
        }
    }
}

impl<'de> Deserialize<'de> for AccountEmail {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(Self::new(value))
    }
}

impl From<String> for AccountEmail {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for AccountEmail {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

#[cfg(feature = "full")]
impl poem_openapi::types::ParseFromJSON for AccountEmail {
    fn parse_from_json(value: Option<serde_json::Value>) -> poem_openapi::types::ParseResult<Self> {
        let raw = <String as poem_openapi::types::ParseFromJSON>::parse_from_json(value)
            .map_err(poem_openapi::types::ParseError::propagate)?;
        Ok(Self::new(raw))
    }
}

#[cfg(feature = "full")]
impl poem_openapi::types::ParseFromParameter for AccountEmail {
    fn parse_from_parameter(value: &str) -> poem_openapi::types::ParseResult<Self> {
        Ok(Self::new(value))
    }
}

#[cfg(feature = "full")]
impl poem_openapi::types::ParseFromMultipartField for AccountEmail {
    async fn parse_from_multipart(
        field: Option<poem::web::Field>,
    ) -> poem_openapi::types::ParseResult<Self> {
        let raw =
            <String as poem_openapi::types::ParseFromMultipartField>::parse_from_multipart(field)
                .await
                .map_err(poem_openapi::types::ParseError::propagate)?;
        Ok(Self::new(raw))
    }
}

declare_structs! {
    pub struct Account {
        pub id: AccountId,
        pub revision: AccountRevision,
        pub name: String,
        pub email: AccountEmail,
        pub plan_id: PlanId,
        pub roles: Vec<AccountRole>,
        pub account_root_card_id: CardId
    }

    pub struct AccountSummary {
        pub id: AccountId,
        pub name: String,
        pub email: AccountEmail,
    }

    pub struct AccountCreation {
        pub name: String,
        pub email: AccountEmail,
        pub roles: Vec<AccountRole>
    }

    pub struct AccountUpdate {
        pub current_revision: AccountRevision,
        pub name: Option<String>,
    }

    pub struct AccountSetPlan {
        pub current_revision: AccountRevision,
        pub plan: PlanId
    }
}
