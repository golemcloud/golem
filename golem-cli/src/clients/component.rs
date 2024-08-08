// Copyright 2024 Golem Cloud
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

use crate::model::component::Component;
use crate::model::{ComponentName, GolemError, PathBufOrStdin};
use async_trait::async_trait;
use golem_common::uri::oss::urn::ComponentUrn;

#[async_trait]
pub trait ComponentClient {
    type ProjectContext;

    async fn get_metadata(
        &self,
        component_urn: &ComponentUrn,
        version: u64,
    ) -> Result<Component, GolemError>;
    async fn get_latest_metadata(
        &self,
        component_urn: &ComponentUrn,
    ) -> Result<Component, GolemError>;
    async fn find(
        &self,
        name: Option<ComponentName>,
        project: &Option<Self::ProjectContext>,
    ) -> Result<Vec<Component>, GolemError>;
    async fn add(
        &self,
        name: ComponentName,
        file: PathBufOrStdin,
        project: &Option<Self::ProjectContext>,
    ) -> Result<Component, GolemError>;
    async fn update(
        &self,
        urn: ComponentUrn,
        file: PathBufOrStdin,
    ) -> Result<Component, GolemError>;
}
