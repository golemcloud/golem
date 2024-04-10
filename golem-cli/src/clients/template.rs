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

use crate::model::template::TemplateView;
use crate::model::{GolemError, PathBufOrStdin, RawTemplateId, TemplateName};

#[async_trait]
pub trait TemplateClient {
    async fn find(&self, name: Option<TemplateName>) -> Result<Vec<TemplateView>, GolemError>;
    async fn add(
        &self,
        name: TemplateName,
        file: PathBufOrStdin,
    ) -> Result<TemplateView, GolemError>;
    async fn update(
        &self,
        id: RawTemplateId,
        file: PathBufOrStdin,
    ) -> Result<TemplateView, GolemError>;
}

#[derive(Clone)]
pub struct TemplateClientLive<C: golem_client::api::TemplateClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_client::api::TemplateClient + Sync + Send> TemplateClient for TemplateClientLive<C> {
    async fn find(&self, name: Option<TemplateName>) -> Result<Vec<TemplateView>, GolemError> {
        info!("Getting templates");

        let name = name.map(|n| n.0);

        let templates: Vec<Template> = self.client.get_templates(name.as_deref()).await?;
        let views = templates.iter().map(|c| c.into()).collect();
        Ok(views)
    }

    async fn add(
        &self,
        name: TemplateName,
        path: PathBufOrStdin,
    ) -> Result<TemplateView, GolemError> {
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

        Ok((&template).into())
    }

    async fn update(
        &self,
        id: RawTemplateId,
        path: PathBufOrStdin,
    ) -> Result<TemplateView, GolemError> {
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

        Ok((&template).into())
    }
}
