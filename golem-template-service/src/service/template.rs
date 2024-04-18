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

use std::error::Error;
use std::fmt::Display;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use async_trait::async_trait;
use futures_util::{Stream, StreamExt, TryStreamExt};
use golem_common::model::TemplateId;
use golem_template_service_base::service::template_compilation::TemplateCompilationService;
use golem_template_service_base::service::template_processor::{
    process_template, TemplateProcessingError,
};
use tap::TapFallible;
use tracing::{error, info};

use crate::repo::template::TemplateRepo;
use crate::repo::RepoError;
use crate::service::template_object_store::TemplateObjectStore;
use golem_service_base::model::*;
use golem_service_base::service::template_object_store::GetTemplateStream;

#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("Template already exists: {0}")]
    AlreadyExists(TemplateId),
    #[error("Unknown template id: {0}")]
    UnknownTemplateId(TemplateId),
    #[error("Unknown versioned template id: {0}")]
    UnknownVersionedTemplateId(VersionedTemplateId),
    #[error(transparent)]
    TemplateProcessingError(#[from] TemplateProcessingError),
    #[error("Internal error: {0}")]
    Internal(anyhow::Error),
}

impl TemplateError {
    fn internal<E, C>(error: E, context: C) -> Self
    where
        E: Display + std::fmt::Debug + Send + Sync + 'static,
        C: Display + Send + Sync + 'static,
    {
        TemplateError::Internal(anyhow::Error::msg(error).context(context))
    }
}

impl From<RepoError> for TemplateError {
    fn from(error: RepoError) -> Self {
        TemplateError::Internal(anyhow::Error::msg(error.to_string()))
    }
}

pub struct DownloadTemplateStream(GetTemplateStream);

impl Stream for DownloadTemplateStream {
    type Item = Result<Vec<u8>, TemplateError>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.0)
            .poll_next(cx)
            .map_err(|e| TemplateError::Internal(e))
    }
}

#[async_trait]
pub trait TemplateService {
    async fn create(
        &self,
        template_name: &TemplateName,
        data: Vec<u8>,
    ) -> Result<Template, TemplateError>;

    async fn update(
        &self,
        template_id: &TemplateId,
        data: Vec<u8>,
    ) -> Result<Template, TemplateError>;

    async fn download(
        &self,
        template_id: &TemplateId,
        version: Option<u64>,
    ) -> Result<Vec<u8>, TemplateError>;

    async fn download_stream(
        &self,
        template_id: &TemplateId,
        version: Option<u64>,
    ) -> Result<Box<dyn Stream<Item = Result<Vec<u8>, anyhow::Error>>>, TemplateError>;

    async fn get_protected_data(
        &self,
        template_id: &TemplateId,
        version: Option<u64>,
    ) -> Result<Option<Vec<u8>>, TemplateError>;

    async fn find_by_name(
        &self,
        template_name: Option<TemplateName>,
    ) -> Result<Vec<Template>, TemplateError>;

    async fn get_by_version(
        &self,
        template_id: &VersionedTemplateId,
    ) -> Result<Option<Template>, TemplateError>;

    async fn get_latest_version(
        &self,
        template_id: &TemplateId,
    ) -> Result<Option<Template>, TemplateError>;

    async fn get(&self, template_id: &TemplateId) -> Result<Vec<Template>, TemplateError>;
}

pub struct TemplateServiceDefault {
    template_repo: Arc<dyn TemplateRepo + Sync + Send>,
    object_store: Arc<dyn TemplateObjectStore + Sync + Send>,
    template_compilation: Arc<dyn TemplateCompilationService + Sync + Send>,
}

impl TemplateServiceDefault {
    pub fn new(
        template_repo: Arc<dyn TemplateRepo + Sync + Send>,
        object_store: Arc<dyn TemplateObjectStore + Sync + Send>,
        template_compilation: Arc<dyn TemplateCompilationService + Sync + Send>,
    ) -> Self {
        TemplateServiceDefault {
            template_repo,
            object_store,
            template_compilation,
        }
    }
}

#[async_trait]
impl TemplateService for TemplateServiceDefault {
    async fn create(
        &self,
        template_name: &TemplateName,
        data: Vec<u8>,
    ) -> Result<Template, TemplateError> {
        let tn = template_name.0.clone();
        info!("Creating template  with name {}", tn);

        self.check_new_name(template_name).await?;

        let metadata = process_template(&data)?;

        let template_id = TemplateId::new_v4();

        let versioned_template_id = VersionedTemplateId {
            template_id,
            version: 0,
        };

        let user_template_id = UserTemplateId {
            versioned_template_id: versioned_template_id.clone(),
        };
        let protected_template_id = ProtectedTemplateId {
            versioned_template_id: versioned_template_id.clone(),
        };

        info!(
            "Uploaded template {} version 0 with exports {:?}",
            versioned_template_id.template_id, metadata.exports
        );

        let template_size: i32 = data
            .len()
            .try_into()
            .map_err(|e| TemplateError::internal(e, "Failed to convert data length"))?;

        tokio::try_join!(
            self.upload_user_template(&user_template_id, data.clone()),
            self.upload_protected_template(&protected_template_id, data)
        )?;

        let template = Template {
            template_name: template_name.clone(),
            template_size,
            metadata,
            versioned_template_id,
            user_template_id,
            protected_template_id,
        };

        self.template_repo.upsert(&template.clone().into()).await?;

        self.template_compilation
            .enqueue_compilation(&template.versioned_template_id.template_id, 0)
            .await;

        Ok(template)
    }

    async fn update(
        &self,
        template_id: &TemplateId,
        data: Vec<u8>,
    ) -> Result<Template, TemplateError> {
        info!("Updating template {}", template_id);

        let metadata = process_template(&data)?;

        let next_template = self
            .template_repo
            .get_latest_version(&template_id.0)
            .await?
            .map(Template::from)
            .map(Template::next_version)
            .ok_or(TemplateError::UnknownTemplateId(template_id.clone()))?;

        info!(
            "Uploaded template {} version {} with exports {:?}",
            template_id, next_template.versioned_template_id.version, metadata.exports
        );

        let template_size: i32 = data
            .len()
            .try_into()
            .map_err(|e| TemplateError::internal(e, "Failed to convert data length"))?;

        tokio::try_join!(
            self.upload_user_template(&next_template.user_template_id, data.clone()),
            self.upload_protected_template(&next_template.protected_template_id, data)
        )?;

        let template = Template {
            template_size,
            metadata,
            ..next_template
        };

        self.template_repo.upsert(&template.clone().into()).await?;

        self.template_compilation
            .enqueue_compilation(template_id, template.versioned_template_id.version)
            .await;

        Ok(template)
    }

    async fn download(
        &self,
        template_id: &TemplateId,
        version: Option<u64>,
    ) -> Result<Vec<u8>, TemplateError> {
        let versioned_template_id = {
            match version {
                Some(version) => VersionedTemplateId {
                    template_id: template_id.clone(),
                    version,
                },
                None => self
                    .template_repo
                    .get_latest_version(&template_id.0)
                    .await?
                    .map(Template::from)
                    .map(|t| t.versioned_template_id)
                    .ok_or(TemplateError::UnknownTemplateId(template_id.clone()))?,
            }
        };

        let id = ProtectedTemplateId {
            versioned_template_id,
        };

        self.object_store
            .get(&self.get_protected_object_store_key(&id))
            .await
            .tap_err(|e| error!("Error downloading template: {}", e))
            .map_err(|e| TemplateError::internal(e.to_string(), "Error downloading template"))
    }

    async fn download_stream(
        &self,
        template_id: &TemplateId,
        version: Option<u64>,
    ) -> Result<Box<dyn Stream<Item = Result<Vec<u8>, anyhow::Error>>>, TemplateError> {
        let versioned_template_id = {
            match version {
                Some(version) => VersionedTemplateId {
                    template_id: template_id.clone(),
                    version,
                },
                None => self
                    .template_repo
                    .get_latest_version(&template_id.0)
                    .await
                    .map_err(TemplateError::from)
                    .and_then(|r| {
                        r.map(Template::from)
                            .map(|t| t.versioned_template_id)
                            .ok_or(TemplateError::UnknownTemplateId(template_id.clone()))
                    })?,
            }
        };

        let id = ProtectedTemplateId {
            versioned_template_id,
        };
        let download_stream = self
            .object_store
            .get_stream(&self.get_protected_object_store_key(&id))
            .await;

        Ok(Box::new(download_stream))
        // todo!()
    }

    async fn get_protected_data(
        &self,
        template_id: &TemplateId,
        version: Option<u64>,
    ) -> Result<Option<Vec<u8>>, TemplateError> {
        info!(
            "Getting template  {} version {} data",
            template_id,
            version.map_or("N/A".to_string(), |v| v.to_string())
        );

        let latest_template = self
            .template_repo
            .get_latest_version(&template_id.0)
            .await?;

        let v = match latest_template {
            Some(template) => match version {
                Some(v) if v <= template.version as u64 => v,
                None => template.version as u64,
                _ => {
                    return Ok(None);
                }
            },
            None => {
                return Ok(None);
            }
        };

        let versioned_template_id = VersionedTemplateId {
            template_id: template_id.clone(),
            version: v,
        };

        let protected_id = ProtectedTemplateId {
            versioned_template_id,
        };

        let object_key = self.get_protected_object_store_key(&protected_id);

        let result = self
            .object_store
            .get(&object_key)
            .await
            .tap_err(|e| error!("Error retrieving template: {}", e))
            .map_err(|e| TemplateError::internal(e.to_string(), "Error retrieving template"))?;

        Ok(Some(result))
    }

    async fn find_by_name(
        &self,
        template_name: Option<TemplateName>,
    ) -> Result<Vec<Template>, TemplateError> {
        let tn = template_name.clone().map_or("N/A".to_string(), |n| n.0);
        info!("Getting template name {}", tn);

        let result = match template_name {
            Some(name) => self.template_repo.get_by_name(&name.0).await?,
            None => self.template_repo.get_all().await?,
        };

        Ok(result.into_iter().map(|t| t.into()).collect())
    }

    async fn get(&self, template_id: &TemplateId) -> Result<Vec<Template>, TemplateError> {
        info!("Getting template {}", template_id);
        let result = self.template_repo.get(&template_id.0).await?;

        Ok(result.into_iter().map(|t| t.into()).collect())
    }

    async fn get_by_version(
        &self,
        template_id: &VersionedTemplateId,
    ) -> Result<Option<Template>, TemplateError> {
        info!(
            "Getting template {} version {}",
            template_id.template_id, template_id.version
        );

        let result = self
            .template_repo
            .get_by_version(&template_id.template_id.0, template_id.version)
            .await?;
        Ok(result.map(|t| t.into()))
    }

    async fn get_latest_version(
        &self,
        template_id: &TemplateId,
    ) -> Result<Option<Template>, TemplateError> {
        info!("Getting template {} latest version", template_id);
        let result = self
            .template_repo
            .get_latest_version(&template_id.0)
            .await?;
        Ok(result.map(|t| t.into()))
    }
}

impl TemplateServiceDefault {
    async fn check_new_name(&self, template_name: &TemplateName) -> Result<(), TemplateError> {
        let existing_templates = self
            .template_repo
            .get_by_name(&template_name.0)
            .await
            .tap_err(|e| error!("Error getting existing templates: {}", e))?;

        existing_templates
            .into_iter()
            .next()
            .map(Template::from)
            .map_or(Ok(()), |t| {
                Err(TemplateError::AlreadyExists(
                    t.versioned_template_id.template_id,
                ))
            })
    }

    fn get_user_object_store_key(&self, id: &UserTemplateId) -> String {
        id.slug()
    }

    fn get_protected_object_store_key(&self, id: &ProtectedTemplateId) -> String {
        id.slug()
    }

    async fn upload_user_template(
        &self,
        user_template_id: &UserTemplateId,
        data: Vec<u8>,
    ) -> Result<(), TemplateError> {
        info!("Uploading user template: {:?}", user_template_id);

        self.object_store
            .put(&self.get_user_object_store_key(user_template_id), data)
            .await
            .map_err(|e| TemplateError::internal(e.to_string(), "Failed to upload user template"))
    }

    async fn upload_protected_template(
        &self,
        protected_template_id: &ProtectedTemplateId,
        data: Vec<u8>,
    ) -> Result<(), TemplateError> {
        info!("Uploading protected template: {:?}", protected_template_id);

        self.object_store
            .put(
                &self.get_protected_object_store_key(protected_template_id),
                data,
            )
            .await
            .map_err(|e| {
                TemplateError::internal(e.to_string(), "Failed to upload protected template")
            })
    }
}

#[derive(Default)]
pub struct TemplateServiceNoOp {}

#[async_trait]
impl TemplateService for TemplateServiceNoOp {
    async fn create(
        &self,
        _template_name: &TemplateName,
        _data: Vec<u8>,
    ) -> Result<Template, TemplateError> {
        let fake_template = Template {
            template_name: TemplateName("fake".to_string()),
            template_size: 0,
            metadata: TemplateMetadata {
                exports: vec![],
                producers: vec![],
            },
            versioned_template_id: VersionedTemplateId {
                template_id: TemplateId::new_v4(),
                version: 0,
            },
            user_template_id: UserTemplateId {
                versioned_template_id: VersionedTemplateId {
                    template_id: TemplateId::new_v4(),
                    version: 0,
                },
            },
            protected_template_id: ProtectedTemplateId {
                versioned_template_id: VersionedTemplateId {
                    template_id: TemplateId::new_v4(),
                    version: 0,
                },
            },
        };

        Ok(fake_template)
    }

    async fn update(
        &self,
        _template_id: &TemplateId,
        _data: Vec<u8>,
    ) -> Result<Template, TemplateError> {
        let fake_template = Template {
            template_name: TemplateName("fake".to_string()),
            template_size: 0,
            metadata: TemplateMetadata {
                exports: vec![],
                producers: vec![],
            },
            versioned_template_id: VersionedTemplateId {
                template_id: TemplateId::new_v4(),
                version: 0,
            },
            user_template_id: UserTemplateId {
                versioned_template_id: VersionedTemplateId {
                    template_id: TemplateId::new_v4(),
                    version: 0,
                },
            },
            protected_template_id: ProtectedTemplateId {
                versioned_template_id: VersionedTemplateId {
                    template_id: TemplateId::new_v4(),
                    version: 0,
                },
            },
        };

        Ok(fake_template)
    }

    async fn download(
        &self,
        _template_id: &TemplateId,
        _version: Option<u64>,
    ) -> Result<Vec<u8>, TemplateError> {
        Ok(vec![])
    }

    async fn download_stream(
        &self,
        _template_id: &TemplateId,
        _version: Option<u64>,
    ) -> Result<Box<dyn Stream<Item = Result<Vec<u8>, anyhow::Error>>>, TemplateError> {
        Ok(Box::new(futures_util::stream::empty()))
    }

    async fn get_protected_data(
        &self,
        _template_id: &TemplateId,
        _version: Option<u64>,
    ) -> Result<Option<Vec<u8>>, TemplateError> {
        Ok(None)
    }

    async fn find_by_name(
        &self,
        _template_name: Option<TemplateName>,
    ) -> Result<Vec<Template>, TemplateError> {
        Ok(vec![])
    }

    async fn get_by_version(
        &self,
        _template_id: &VersionedTemplateId,
    ) -> Result<Option<Template>, TemplateError> {
        Ok(None)
    }

    async fn get_latest_version(
        &self,
        _template_id: &TemplateId,
    ) -> Result<Option<Template>, TemplateError> {
        Ok(None)
    }

    async fn get(&self, _template_id: &TemplateId) -> Result<Vec<Template>, TemplateError> {
        Ok(vec![])
    }
}
