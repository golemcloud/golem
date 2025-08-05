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

use openapiv3::OpenAPI;
use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
use poem_openapi::types::{ParseError, ParseFromJSON, ParseFromYAML, ParseResult};
use serde_json::Value;
use std::borrow::Cow;

pub struct OpenApiHttpApiDefinition(pub OpenAPI);

impl ParseFromJSON for OpenApiHttpApiDefinition {
    fn parse_from_json(value: Option<serde_json::Value>) -> ParseResult<Self> {
        match value {
            Some(value) => match serde_json::from_value::<openapiv3::OpenAPI>(value) {
                Ok(openapi) => Ok(OpenApiHttpApiDefinition(openapi)),
                Err(e) => Err(ParseError::<Self>::custom(format!(
                    "Failed to parse OpenAPI: {e}"
                ))),
            },

            _ => Err(ParseError::<Self>::custom(
                "OpenAPI spec missing".to_string(),
            )),
        }
    }
}

impl ParseFromYAML for OpenApiHttpApiDefinition {
    fn parse_from_yaml(value: Option<Value>) -> ParseResult<Self> {
        match value {
            Some(value) => match serde_json::from_value::<openapiv3::OpenAPI>(value) {
                Ok(openapi) => Ok(OpenApiHttpApiDefinition(openapi)),
                Err(e) => Err(ParseError::<Self>::custom(format!(
                    "Failed to parse OpenAPI: {e}"
                ))),
            },

            _ => Err(ParseError::<Self>::custom(
                "OpenAPI spec missing".to_string(),
            )),
        }
    }
}

impl poem_openapi::types::Type for OpenApiHttpApiDefinition {
    const IS_REQUIRED: bool = true;

    type RawValueType = Self;

    type RawElementValueType = Self;

    fn name() -> Cow<'static, str> {
        "OpenApiDefinition".into()
    }

    fn schema_ref() -> MetaSchemaRef {
        MetaSchemaRef::Inline(Box::new(MetaSchema {
            title: Some("API definition in OpenAPI format".to_string()),
            description: Some("API definition in OpenAPI format with required custom extensions"),
            ..MetaSchema::new("OpenAPI+WorkerBridgeCustomExtension")
        }))
    }

    fn as_raw_value(&self) -> Option<&Self::RawValueType> {
        Some(self)
    }

    fn raw_element_iter<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a Self::RawElementValueType> + 'a> {
        Box::new(self.as_raw_value().into_iter())
    }
}
