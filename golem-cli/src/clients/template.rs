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

use std::io::Read;

use async_trait::async_trait;
use golem_client::model::Template;

use tokio::fs::File;
use tracing::info;

use crate::model::{GolemError, PathBufOrStdin, TemplateId, TemplateName};

#[async_trait]
pub trait TemplateClient {
    async fn get_metadata(
        &self,
        template_id: &TemplateId,
        version: u64,
    ) -> Result<Template, GolemError>;
    async fn get_latest_metadata(&self, template_id: &TemplateId) -> Result<Template, GolemError>;
    async fn find(&self, name: Option<TemplateName>) -> Result<Vec<Template>, GolemError>;
    async fn add(&self, name: TemplateName, file: PathBufOrStdin) -> Result<Template, GolemError>;
    async fn update(&self, id: TemplateId, file: PathBufOrStdin) -> Result<Template, GolemError>;
}

#[derive(Clone)]
pub struct TemplateClientLive<C: golem_client::api::TemplateClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_client::api::TemplateClient + Sync + Send> TemplateClient for TemplateClientLive<C> {
    async fn get_metadata(
        &self,
        template_id: &TemplateId,
        version: u64,
    ) -> Result<Template, GolemError> {
        info!("Getting template version");

        Ok(self
            .client
            .get_template_metadata(&template_id.0, &version.to_string())
            .await?)
    }

    async fn get_latest_metadata(&self, template_id: &TemplateId) -> Result<Template, GolemError> {
        info!("Getting latest template version");

        Ok(self
            .client
            .get_latest_template_metadata(&template_id.0)
            .await?)
    }

    async fn find(&self, name: Option<TemplateName>) -> Result<Vec<Template>, GolemError> {
        info!("Getting templates");

        let name = name.map(|n| n.0);

        Ok(self.client.get_templates(name.as_deref()).await?)
    }

    async fn add(&self, name: TemplateName, path: PathBufOrStdin) -> Result<Template, GolemError> {
        info!("Adding template {name:?} from {path:?}");

        let template = match path {
            PathBufOrStdin::Path(path) => {
                let file = File::open(path)
                    .await
                    .map_err(|e| GolemError(format!("Can't open template file: {e}")))?;

                self.client.create_template(&name.0, file).await?
            }
            PathBufOrStdin::Stdin => {
                let mut bytes = Vec::new();

                let _ = std::io::stdin()
                    .read_to_end(&mut bytes) // TODO: steaming request from stdin
                    .map_err(|e| GolemError(format!("Failed to read stdin: {e:?}")))?;

                self.client.create_template(&name.0, bytes).await?
            }
        };

        Ok(template)
    }

    async fn update(&self, id: TemplateId, path: PathBufOrStdin) -> Result<Template, GolemError> {
        info!("Updating template {id:?} from {path:?}");

        let template = match path {
            PathBufOrStdin::Path(path) => {
                let file = File::open(path)
                    .await
                    .map_err(|e| GolemError(format!("Can't open template file: {e}")))?;

                self.client.update_template(&id.0, file).await?
            }
            PathBufOrStdin::Stdin => {
                let mut bytes = Vec::new();

                let _ = std::io::stdin()
                    .read_to_end(&mut bytes)
                    .map_err(|e| GolemError(format!("Failed to read stdin: {e:?}")))?;

                self.client.update_template(&id.0, bytes).await?
            }
        };

        Ok(template)
    }
}
