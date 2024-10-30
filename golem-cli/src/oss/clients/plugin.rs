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

use crate::clients::plugin::PluginClient;
use crate::model::GolemError;
use crate::oss::model::OssContext;
use async_trait::async_trait;
use golem_common::model::plugin::DefaultPluginScope;
use tracing::info;

#[derive(Debug, Clone)]
pub struct PluginClientLive<C: golem_client::api::PluginClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_client::api::PluginClient + Sync + Send> PluginClient for PluginClientLive<C> {
    type ProjectContext = OssContext;
    type PluginDefinition =
        golem_client::model::PluginDefinitionDefaultPluginOwnerDefaultPluginScope;
    type PluginScope = DefaultPluginScope;

    async fn list_plugins(
        &self,
        scope: Option<DefaultPluginScope>,
    ) -> Result<Vec<Self::PluginDefinition>, GolemError> {
        info!("Getting registered plugins");

        Ok(self.client.list_plugins(scope.as_ref()).await?)
    }
}
