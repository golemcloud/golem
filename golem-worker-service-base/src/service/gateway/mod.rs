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

use async_trait::async_trait;
use golem_common::model::ComponentId;
use golem_service_base::model::Component;
use golem_service_base::model::ComponentName;

pub mod api_definition;
pub mod api_definition_validator;
pub mod api_deployment;
pub mod http_api_definition_validator;
pub mod security_scheme;

#[derive(Debug, Clone)]
pub struct ComponentView {
    pub id: ComponentId,
    pub name: ComponentName,
    pub latest_version: u64,
}

impl From<Component> for ComponentView {
    fn from(value: Component) -> Self {
        Self {
            id: value.versioned_component_id.component_id,
            name: value.component_name,
            latest_version: value.versioned_component_id.version,
        }
    }
}

#[async_trait]
pub trait ConversionContext: Send + Sync {
    async fn component_by_name(&self, name: &ComponentName) -> Result<ComponentView, String>;
    async fn component_by_id(&self, component_id: &ComponentId) -> Result<ComponentView, String>;

    fn boxed<'a>(self) -> BoxConversionContext<'a>
    where
        Self: Sized + 'a,
    {
        Box::new(self)
    }
}

pub type BoxConversionContext<'a> = Box<dyn ConversionContext + 'a>;
