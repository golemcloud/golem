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

use crate::cloud::model::ProjectRef;
use crate::cloud::service::certificate::CertificateService;
use clap::Subcommand;
use uuid::Uuid;

use crate::model::{GolemError, GolemResult, PathBufOrStdin};

#[derive(Subcommand, Debug)]
#[command()]
pub enum CertificateSubcommand {
    #[command()]
    Get {
        #[command(flatten)]
        project_ref: ProjectRef,
        #[arg(value_name = "certificate-id", value_hint = clap::ValueHint::Other)]
        certificate_id: Option<Uuid>,
    },
    #[command()]
    Add {
        #[command(flatten)]
        project_ref: ProjectRef,

        #[arg(short, long, value_hint = clap::ValueHint::Other)]
        domain_name: String,

        #[arg(short = 'b', long, value_name = "file", value_hint = clap::ValueHint::FilePath)]
        certificate_body: PathBufOrStdin,

        #[arg(short = 'k', long, value_name = "file", value_hint = clap::ValueHint::FilePath)]
        certificate_private_key: PathBufOrStdin,
    },
    #[command()]
    Delete {
        #[command(flatten)]
        project_ref: ProjectRef,
        #[arg(value_name = "certificate-id", value_hint = clap::ValueHint::Other)]
        certificate_id: Uuid,
    },
}

impl CertificateSubcommand {
    pub async fn handle(
        self,
        service: &(dyn CertificateService + Send + Sync),
    ) -> Result<GolemResult, GolemError> {
        match self {
            CertificateSubcommand::Get {
                project_ref,
                certificate_id,
            } => service.get(project_ref, certificate_id).await,
            CertificateSubcommand::Add {
                project_ref,
                domain_name,
                certificate_body,
                certificate_private_key,
            } => {
                service
                    .add(
                        project_ref,
                        domain_name,
                        certificate_body,
                        certificate_private_key,
                    )
                    .await
            }
            CertificateSubcommand::Delete {
                project_ref,
                certificate_id,
            } => service.delete(project_ref, certificate_id).await,
        }
    }
}
