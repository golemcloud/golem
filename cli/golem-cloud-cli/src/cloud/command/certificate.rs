use crate::cloud::model::ProjectRef;
use crate::cloud::service::certificate::CertificateService;
use clap::Subcommand;
use uuid::Uuid;

use golem_cli::model::{GolemError, GolemResult, PathBufOrStdin};

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
