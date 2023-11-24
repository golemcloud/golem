use std::fs::File;
use std::io;
use std::io::{BufReader, Read};

use async_trait::async_trait;
use clap::Subcommand;
use golem_gateway_client::models::CertificateRequest;

use crate::clients::gateway::certificate::CertificateClient;
use crate::clients::project::ProjectClient;
use crate::clients::CloudAuthentication;
use crate::model::{GolemError, GolemResult, PathBufOrStdin, ProjectRef};

#[derive(Subcommand, Debug)]
#[command()]
pub enum CertificateSubcommand {
    #[command()]
    Get {
        #[command(flatten)]
        project_ref: ProjectRef,
        #[arg(value_name = "certificate-id", value_hint = clap::ValueHint::Other)]
        certificate_id: Option<String>,
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
        certificate_id: String,
    },
}

#[async_trait]
pub trait CertificateHandler {
    async fn handle(
        &self,
        auth: &CloudAuthentication,
        command: CertificateSubcommand,
    ) -> Result<GolemResult, GolemError>;
}

pub struct CertificateHandlerLive<
    'p,
    C: CertificateClient + Sync + Send,
    P: ProjectClient + Sync + Send,
> {
    pub client: C,
    pub projects: &'p P,
}

fn read_as_string<R: Read>(mut r: R, source: &str) -> Result<String, GolemError> {
    let mut result = String::new();

    r.read_to_string(&mut result)
        .map_err(|e| GolemError(format!("Failed to read {source} as String: ${e}")))?;

    Ok(result)
}

fn read_path_or_stdin_as_string(path_or_stdin: PathBufOrStdin) -> Result<String, GolemError> {
    match path_or_stdin {
        PathBufOrStdin::Path(path) => {
            let file = File::open(&path)
                .map_err(|e| GolemError(format!("Failed to open file {path:?}: {e}")))?;

            let reader = BufReader::new(file);

            read_as_string(reader, &format!("file `{path:?}`"))
        }
        PathBufOrStdin::Stdin => read_as_string(io::stdin(), "stdin"),
    }
}

#[async_trait]
impl<'p, C: CertificateClient + Sync + Send, P: ProjectClient + Sync + Send> CertificateHandler
    for CertificateHandlerLive<'p, C, P>
{
    async fn handle(
        &self,
        auth: &CloudAuthentication,
        command: CertificateSubcommand,
    ) -> Result<GolemResult, GolemError> {
        match command {
            CertificateSubcommand::Get {
                project_ref,
                certificate_id,
            } => {
                let project_id = self
                    .projects
                    .resolve_id_or_default(project_ref, auth)
                    .await?;

                let res = self
                    .client
                    .get(project_id, certificate_id.as_deref())
                    .await?;

                Ok(GolemResult::Ok(Box::new(res)))
            }
            CertificateSubcommand::Add {
                project_ref,
                domain_name,
                certificate_body,
                certificate_private_key,
            } => {
                let project_id = self
                    .projects
                    .resolve_id_or_default(project_ref, auth)
                    .await?;

                let request = CertificateRequest {
                    project_id: project_id.0,
                    domain_name,
                    certificate_body: read_path_or_stdin_as_string(certificate_body)?,
                    certificate_private_key: read_path_or_stdin_as_string(certificate_private_key)?,
                };

                let res = self.client.create(request).await?;

                Ok(GolemResult::Ok(Box::new(res)))
            }
            CertificateSubcommand::Delete {
                project_ref,
                certificate_id,
            } => {
                let project_id = self
                    .projects
                    .resolve_id_or_default(project_ref, auth)
                    .await?;
                let res = self.client.delete(project_id, &certificate_id).await?;
                Ok(GolemResult::Ok(Box::new(res)))
            }
        }
    }
}
