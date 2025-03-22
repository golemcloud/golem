// Copyright 2024-2025 Golem Cloud
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

use async_trait::async_trait;
use golem_common::model::ComponentId;
use golem_service_base::model::ComponentName;
pub mod api_definition;
pub mod api_definition_validator;
pub mod api_deployment;
pub mod http_api_definition_validator;
pub mod security_scheme;

#[async_trait]
pub trait ConversionContext: Send + Sync {
    async fn resolve_component_id(&self, name: &ComponentName) -> Result<ComponentId, String>;
    async fn get_component_name(&self, component_id: &ComponentId)
        -> Result<ComponentName, String>;

    fn boxed<'a>(self) -> BoxConversionContext<'a>
    where
        Self: Sized + 'a,
    {
        Box::new(self)
    }
}

pub type BoxConversionContext<'a> = Box<dyn ConversionContext + 'a>;
