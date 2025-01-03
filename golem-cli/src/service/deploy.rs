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

use crate::model::{Format, GolemError, GolemResult, WorkerName, WorkerUpdateMode};
use crate::service::component::ComponentService;
use crate::service::worker::WorkerService;
use async_trait::async_trait;
use golem_common::uri::oss::uri::ComponentUri;
use golem_common::uri::oss::urn::WorkerUrn;
use inquire::Confirm;
use std::fmt::Display;
use std::sync::Arc;
use tracing::{debug, info};

/// Higher-level deployment operations implemented on top of the underlying services
#[async_trait]
pub trait DeployService {
    type ProjectContext: Send + Sync;

    async fn try_update_all_workers(
        &self,
        component_uri: ComponentUri,
        project: Option<Self::ProjectContext>,
        mode: WorkerUpdateMode,
    ) -> Result<GolemResult, GolemError>;

    async fn redeploy(
        &self,
        component_uri: ComponentUri,
        project: Option<Self::ProjectContext>,
        non_interactive: bool,
        format: Format,
    ) -> Result<GolemResult, GolemError>;
}

pub struct DeployServiceLive<ProjectContext> {
    pub component_service: Arc<dyn ComponentService<ProjectContext = ProjectContext> + Send + Sync>,
    pub worker_service: Arc<dyn WorkerService<ProjectContext = ProjectContext> + Send + Sync>,
}

#[async_trait]
impl<ProjectContext: Display + Send + Sync> DeployService for DeployServiceLive<ProjectContext> {
    type ProjectContext = ProjectContext;

    async fn try_update_all_workers(
        &self,
        component_uri: ComponentUri,
        project: Option<Self::ProjectContext>,
        mode: WorkerUpdateMode,
    ) -> Result<GolemResult, GolemError> {
        let component_urn = self
            .component_service
            .resolve_uri(component_uri, &project)
            .await?;
        let component = self
            .component_service
            .get_latest_metadata(&component_urn)
            .await?;
        let target_version = component.versioned_component_id.version;

        info!(
            "Attempting to update all workers of component {} to version {}",
            component_urn, target_version
        );

        self.worker_service
            .update_many_by_urn(component_urn, None, target_version, mode)
            .await
    }

    async fn redeploy(
        &self,
        component_uri: ComponentUri,
        project: Option<Self::ProjectContext>,
        non_interactive: bool,
        format: Format,
    ) -> Result<GolemResult, GolemError> {
        let component_urn = self
            .component_service
            .resolve_uri(component_uri, &project)
            .await?;
        let component = self
            .component_service
            .get_latest_metadata(&component_urn)
            .await?;
        let target_version = component.versioned_component_id.version;
        let known_workers = self
            .worker_service
            .list_worker_metadata(&component_urn, None, Some(true))
            .await?;

        if format == Format::Text && !non_interactive {
            let answer = Confirm::new(&format!(
                "Do you want to recreate all the {} workers of component {}?",
                known_workers.len(),
                component_urn
            ))
            .with_default(false)
            .with_help_message("The workers state will be lost!")
            .prompt();

            match answer {
                Ok(true) => debug!("Operation confirmed by the user"),
                Ok(false) => return Ok(GolemResult::Str("Operation canceled by the user".to_string())),
                Err(error) => return Err(GolemError(format!("Error while asking for confirmation: {}; Use the --non-interactive (-y) flag to bypass it.", error))),
            }
        } else if !non_interactive {
            return Err(GolemError(
                "Pass the --non-interactive (-y) flag or use text format for manual confirmation"
                    .to_string(),
            ));
        }

        info!("Deleting all workers of component {}", component_urn);
        for worker in &known_workers {
            let worker_name = &worker.worker_id.worker_name;
            info!("Deleting worker {worker_name}");

            let worker_urn = WorkerUrn {
                id: worker.worker_id.clone().into_target_worker_id(),
            };
            self.worker_service.delete_by_urn(worker_urn).await?;
        }

        info!(
            "Recreating all workers of component {} using version {target_version}",
            component_urn
        );
        for worker in known_workers {
            info!("Recreating worker {}", worker.worker_id.worker_name);
            self.worker_service
                .add_by_urn(
                    component_urn.clone(),
                    WorkerName(worker.worker_id.worker_name.clone()),
                    worker
                        .env
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect::<Vec<_>>(),
                    worker.args.clone(),
                )
                .await?;
        }

        Ok(GolemResult::Str(
            "Operation completed successfully".to_string(),
        ))
    }
}
