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

use super::{ComponentMetadata, ComponentMetadataInnerData};
use std::sync::Arc;

impl poem_openapi::types::Type for ComponentMetadata {
    const IS_REQUIRED: bool =
        <ComponentMetadataInnerData as poem_openapi::types::Type>::IS_REQUIRED;
    type RawValueType = <ComponentMetadataInnerData as poem_openapi::types::Type>::RawValueType;
    type RawElementValueType =
        <ComponentMetadataInnerData as poem_openapi::types::Type>::RawElementValueType;

    fn name() -> std::borrow::Cow<'static, str> {
        <ComponentMetadataInnerData as poem_openapi::types::Type>::name()
    }

    fn schema_ref() -> poem_openapi::registry::MetaSchemaRef {
        <ComponentMetadataInnerData as poem_openapi::types::Type>::schema_ref()
    }

    fn register(registry: &mut poem_openapi::registry::Registry) {
        <ComponentMetadataInnerData as poem_openapi::types::Type>::register(registry);
    }

    fn as_raw_value(&self) -> Option<&Self::RawValueType> {
        <ComponentMetadataInnerData as poem_openapi::types::Type>::as_raw_value(&self.data)
    }

    fn raw_element_iter<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a Self::RawElementValueType> + 'a> {
        <ComponentMetadataInnerData as poem_openapi::types::Type>::raw_element_iter(&self.data)
    }
}

impl poem_openapi::types::IsObjectType for ComponentMetadata {}

impl poem_openapi::types::ParseFromJSON for ComponentMetadata {
    fn parse_from_json(value: Option<serde_json::Value>) -> poem_openapi::types::ParseResult<Self> {
        let data =
            ComponentMetadataInnerData::parse_from_json(value).map_err(|err| err.propagate())?;
        Ok(Self {
            data: Arc::new(data),
        })
    }
}

impl poem_openapi::types::ToJSON for ComponentMetadata {
    fn to_json(&self) -> Option<serde_json::Value> {
        self.data.to_json()
    }
}

impl poem_openapi::types::ParseFromXML for ComponentMetadata {
    fn parse_from_xml(value: Option<serde_json::Value>) -> poem_openapi::types::ParseResult<Self> {
        let data =
            ComponentMetadataInnerData::parse_from_xml(value).map_err(|err| err.propagate())?;
        Ok(Self {
            data: Arc::new(data),
        })
    }
}

impl poem_openapi::types::ToXML for ComponentMetadata {
    fn to_xml(&self) -> Option<serde_json::Value> {
        self.data.to_xml()
    }
}

impl poem_openapi::types::ParseFromYAML for ComponentMetadata {
    fn parse_from_yaml(value: Option<serde_json::Value>) -> poem_openapi::types::ParseResult<Self> {
        let data =
            ComponentMetadataInnerData::parse_from_yaml(value).map_err(|err| err.propagate())?;
        Ok(Self {
            data: Arc::new(data),
        })
    }
}

impl poem_openapi::types::ToYAML for ComponentMetadata {
    fn to_yaml(&self) -> Option<serde_json::Value> {
        self.data.to_yaml()
    }
}
