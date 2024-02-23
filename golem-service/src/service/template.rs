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

use std::fmt::Display;
use std::sync::Arc;

use async_trait::async_trait;
use golem_common::model::TemplateId;
use golem_wasm_ast::analysis::{
    AnalysedExport, AnalysedFunction, AnalysisContext, AnalysisFailure,
};
use golem_wasm_ast::component::Component;
use golem_wasm_ast::IgnoreAllButMetadata;
use tap::TapFallible;
use tracing::{error, info};

use crate::repo::template::TemplateRepo;
use crate::repo::RepoError;
use crate::service::template_object_store::TemplateObjectStore;
use golem_service_base::model::*;

#[derive(Debug, Clone)]
pub enum TemplateError {
    AlreadyExists(TemplateId),
    UnknownTemplateId(TemplateId),
    UnknownVersionedTemplateId(VersionedTemplateId),
    Internal(String),
    IOError(String),
    // TODO: processing error? more detail?
    TemplateProcessingError(String),
}

impl Display for TemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            TemplateError::AlreadyExists(ref template_id) => {
                write!(f, "Template already exists: {}", template_id)
            }
            TemplateError::UnknownTemplateId(ref template_id) => {
                write!(f, "Unknown template id: {}", template_id)
            }
            TemplateError::UnknownVersionedTemplateId(ref template_id) => {
                write!(f, "Unknown versioned template id: {}", template_id)
            }
            TemplateError::Internal(ref error) => write!(f, "Internal error: {}", error),
            TemplateError::IOError(ref error) => write!(f, "IO error: {}", error),
            TemplateError::TemplateProcessingError(ref error) => {
                write!(f, "Template processing error: {}", error)
            }
        }
    }
}

impl TemplateError {
    pub fn internal<T: Display>(error: T) -> Self {
        TemplateError::Internal(error.to_string())
    }
}

impl From<RepoError> for TemplateError {
    fn from(error: RepoError) -> Self {
        TemplateError::internal(error)
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
        version: Option<i32>,
    ) -> Result<Vec<u8>, TemplateError>;

    async fn get_protected_data(
        &self,
        template_id: &TemplateId,
        version: Option<i32>,
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
}

impl TemplateServiceDefault {
    pub fn new(
        template_repo: Arc<dyn TemplateRepo + Sync + Send>,
        object_store: Arc<dyn TemplateObjectStore + Sync + Send>,
    ) -> Self {
        TemplateServiceDefault {
            template_repo,
            object_store,
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

        let metadata = self.process_template(&data)?;

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

        let template_size: i32 = data.len().try_into().map_err(|e| {
            TemplateError::internal(format!("Failed to convert data length: {}", e))
        })?;

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

        Ok(template)
    }

    async fn update(
        &self,
        template_id: &TemplateId,
        data: Vec<u8>,
    ) -> Result<Template, TemplateError> {
        info!("Updating template {}", template_id);

        let metadata = self.process_template(&data)?;

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

        let template_size: i32 = data.len().try_into().map_err(|e| {
            TemplateError::internal(format!("Failed to convert data length: {}", e))
        })?;

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

        Ok(template)
    }

    async fn download(
        &self,
        template_id: &TemplateId,
        version: Option<i32>,
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
            .map_err(|e| TemplateError::IOError(e.to_string()))
    }

    async fn get_protected_data(
        &self,
        template_id: &TemplateId,
        version: Option<i32>,
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
                Some(v) if v <= template.version => v,
                None => template.version,
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
            .map_err(|e| TemplateError::internal(e.to_string()))?;

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

    fn process_template(&self, data: &[u8]) -> Result<TemplateMetadata, TemplateError> {
        let component = Component::<IgnoreAllButMetadata>::from_bytes(data)
            .map_err(|e| TemplateError::TemplateProcessingError(e.to_string()))?;

        let producers = component
            .get_all_producers()
            .into_iter()
            .map(|producers| producers.into())
            .collect::<Vec<_>>();

        let state = AnalysisContext::new(component);

        let mut exports = state.get_top_level_exports().map_err(|e| {
            TemplateError::TemplateProcessingError(format!(
                "Error getting top level exports: {}",
                match e {
                    AnalysisFailure::Failed(e) => e,
                }
            ))
        })?;

        self.add_resource_drops(&mut exports);

        let exports = exports
            .into_iter()
            .map(|export| export.into())
            .collect::<Vec<_>>();

        Ok(TemplateMetadata { exports, producers })
    }

    fn add_resource_drops(&self, exports: &mut Vec<AnalysedExport>) {
        // Components are not exporting explicit drop functions for exported resources, but
        // worker executor does. So we keep golem-wasm-ast as an universal library and extend
        // its result with the explicit drops here, for each resource, identified by an exported
        // constructor.

        let mut to_add = Vec::new();
        for export in exports.iter_mut() {
            match export {
                AnalysedExport::Function(fun) => {
                    if fun.is_constructor() {
                        let drop_name = fun.name.replace("[constructor]", "[drop]");
                        to_add.push(AnalysedExport::Function(AnalysedFunction {
                            name: drop_name,
                            ..fun.clone()
                        }));
                    }
                }
                AnalysedExport::Instance(instance) => {
                    let mut to_add = Vec::new();
                    for fun in &instance.funcs {
                        if fun.is_constructor() {
                            let drop_name = fun.name.replace("[constructor]", "[drop]");
                            to_add.push(AnalysedFunction {
                                name: drop_name,
                                ..fun.clone()
                            });
                        }
                    }
                    instance.funcs.extend(to_add.into_iter());
                }
            }
        }

        exports.extend(to_add);
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
            .map_err(|e| {
                let message = format!("Failed to upload user template: {}", e);
                error!("{}", message);
                TemplateError::IOError(message)
            })
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
                let message = format!("Failed to upload protected template: {}", e);
                error!("{}", message);
                TemplateError::IOError(message)
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
        _version: Option<i32>,
    ) -> Result<Vec<u8>, TemplateError> {
        Ok(vec![])
    }

    async fn get_protected_data(
        &self,
        _template_id: &TemplateId,
        _version: Option<i32>,
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
