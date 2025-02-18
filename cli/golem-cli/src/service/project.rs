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

use crate::model::GolemError;
use crate::oss::model::OssContext;
use async_trait::async_trait;

#[async_trait]
pub trait ProjectResolver<ProjectRef: Send + Sync + 'static, ProjectContext> {
    async fn resolve_id_or_default(
        &self,
        project_ref: ProjectRef,
    ) -> Result<ProjectContext, GolemError>;

    async fn resolve_id_or_default_opt(
        &self,
        project_ref: Option<ProjectRef>,
    ) -> Result<Option<ProjectContext>, GolemError> {
        match project_ref {
            None => Ok(None),
            Some(project_ref) => Ok(Some(self.resolve_id_or_default(project_ref).await?)),
        }
    }
}

pub struct ProjectResolverOss {}

impl ProjectResolverOss {
    pub const DUMMY: ProjectResolverOss = ProjectResolverOss {};
}

#[async_trait]
impl ProjectResolver<OssContext, OssContext> for ProjectResolverOss {
    async fn resolve_id_or_default(
        &self,
        _project_ref: OssContext,
    ) -> Result<OssContext, GolemError> {
        Ok(OssContext::EMPTY)
    }
}
