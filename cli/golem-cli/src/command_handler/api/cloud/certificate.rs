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

use crate::command::api::cloud::certificate::ApiCertificateSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::AnyhowMapServiceError;
use crate::error::NonSuccessfulExit;
use crate::model::text::certificate::{CertificateListView, CertificateNewView};
use crate::model::text::fmt::log_error;
use crate::model::{PathBufOrStdin, ProjectName};
use anyhow::bail;
use golem_cloud_client::api::ApiCertificateClient;
use golem_cloud_client::model::CertificateRequest;
use golem_wasm_rpc_stubgen::log::log_warn_action;
use std::sync::Arc;
use uuid::Uuid;

pub struct ApiCloudCertificateCommandHandler {
    ctx: Arc<Context>,
}

impl ApiCloudCertificateCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, command: ApiCertificateSubcommand) -> anyhow::Result<()> {
        match command {
            ApiCertificateSubcommand::Get {
                project_name,
                certificate_id,
            } => self.cmd_get(project_name, certificate_id).await,
            ApiCertificateSubcommand::New {
                project_name,
                domain_name,
                certificate_body,
                certificate_private_key,
            } => {
                self.cmd_new(
                    project_name,
                    domain_name,
                    certificate_body,
                    certificate_private_key,
                )
                .await
            }
            ApiCertificateSubcommand::Delete {
                project_name,
                certificate_id,
            } => self.cmd_delete(project_name, certificate_id).await,
        }
    }

    async fn cmd_get(
        &self,
        project_name: ProjectName,
        certificate_id: Option<Uuid>,
    ) -> anyhow::Result<()> {
        let certificates = self
            .ctx
            .golem_clients_cloud()
            .await?
            .api_certificate
            .get_certificates(
                &self
                    .ctx
                    .cloud_project_handler()
                    .select_project(None, &project_name)
                    .await?
                    .project_id
                    .0,
                certificate_id.as_ref(),
            )
            .await
            .map_service_error()?;

        self.ctx
            .log_handler()
            .log_view(&CertificateListView(certificates));

        Ok(())
    }

    async fn cmd_new(
        &self,
        project_name: ProjectName,
        domain_name: String,
        certificate_body: PathBufOrStdin,
        certificate_private_key: PathBufOrStdin,
    ) -> anyhow::Result<()> {
        if certificate_body.is_stdin() && certificate_private_key.is_stdin() {
            log_error("Cannot use STDIN for multiple inputs!");
            bail!(NonSuccessfulExit)
        }

        let certificate = self
            .ctx
            .golem_clients_cloud()
            .await?
            .api_certificate
            .create_certificate(&CertificateRequest {
                project_id: self
                    .ctx
                    .cloud_project_handler()
                    .select_project(None, &project_name)
                    .await?
                    .project_id
                    .0,
                domain_name,
                certificate_body: "".to_string(),
                certificate_private_key: "".to_string(),
            })
            .await
            .map_service_error()?;

        self.ctx
            .log_handler()
            .log_view(&CertificateNewView(certificate));

        Ok(())
    }

    async fn cmd_delete(
        &self,
        project_name: ProjectName,
        certificate_id: Uuid,
    ) -> anyhow::Result<()> {
        self.ctx
            .golem_clients_cloud()
            .await?
            .api_certificate
            .delete_certificate(
                &self
                    .ctx
                    .cloud_project_handler()
                    .select_project(None, &project_name)
                    .await?
                    .project_id
                    .0,
                &certificate_id,
            )
            .await
            .map_service_error()?;

        log_warn_action("Deleted", "certificate");

        Ok(())
    }
}
