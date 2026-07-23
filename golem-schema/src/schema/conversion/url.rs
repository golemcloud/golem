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

use super::{FromSchema, FromSchemaError, IntoSchema, SchemaBuilder, url_from_value, url_to_value};
use crate::schema::metadata::TypeId;
use crate::schema::schema_type::{SchemaType, UrlRestrictions};
use crate::schema::schema_value::SchemaValue;

// A `url::Url` is modelled as the rich `Url` schema type.
impl IntoSchema for ::url::Url {
    fn type_id() -> TypeId {
        TypeId::new("url.Url")
    }
    fn register_in(_b: &mut SchemaBuilder) -> SchemaType {
        SchemaType::url(UrlRestrictions::default())
    }
    fn to_value(&self) -> SchemaValue {
        url_to_value(self.to_string())
    }
}

impl FromSchema for ::url::Url {
    fn from_value(v: &SchemaValue) -> Result<Self, FromSchemaError> {
        let url = url_from_value(v, "Url")?;
        ::url::Url::parse(&url).map_err(|e| FromSchemaError::custom(format!("invalid Url: {e}")))
    }
}
