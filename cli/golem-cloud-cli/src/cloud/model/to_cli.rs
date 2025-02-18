use crate::cloud::model::to_oss::ToOss;
use golem_cli::model::component::Component;

pub trait ToCli<T> {
    fn to_cli(self) -> T;
}

impl<A: ToCli<B>, B> ToCli<Option<B>> for Option<A> {
    fn to_cli(self) -> Option<B> {
        self.map(|v| v.to_cli())
    }
}

impl<A: ToCli<B>, B> ToCli<Vec<B>> for Vec<A> {
    fn to_cli(self) -> Vec<B> {
        self.into_iter().map(|v| v.to_cli()).collect()
    }
}

impl ToCli<golem_cli::model::WorkerMetadata> for golem_cloud_client::model::WorkerMetadata {
    fn to_cli(self) -> golem_cli::model::WorkerMetadata {
        golem_cli::model::WorkerMetadata {
            worker_id: self.worker_id.to_oss(),
            account_id: Some(golem_cli::cloud::AccountId {
                id: self.account_id,
            }),
            args: self.args,
            env: self.env,
            status: self.status.to_oss(),
            component_version: self.component_version,
            retry_count: self.retry_count,
            pending_invocation_count: self.pending_invocation_count,
            updates: self.updates.to_oss(),
            created_at: self.created_at,
            last_error: self.last_error,
            component_size: self.component_size,
            total_linear_memory_size: self.total_linear_memory_size,
            owned_resources: self.owned_resources.to_oss(),
        }
    }
}

impl ToCli<golem_cli::model::WorkersMetadataResponse>
    for golem_cloud_client::model::WorkersMetadataResponse
{
    fn to_cli(self) -> golem_cli::model::WorkersMetadataResponse {
        golem_cli::model::WorkersMetadataResponse {
            cursor: self.cursor.to_oss(),
            workers: self.workers.to_cli(),
        }
    }
}

impl ToCli<golem_cli::model::ApiDeployment> for golem_cloud_client::model::ApiDeployment {
    fn to_cli(self) -> golem_cli::model::ApiDeployment {
        golem_cli::model::ApiDeployment {
            api_definitions: self.api_definitions.to_oss(),
            project_id: Some(self.project_id),
            site: self.site.to_oss(),
            created_at: self.created_at,
        }
    }
}

impl ToCli<golem_cli::model::ApiSecurityScheme> for golem_cloud_client::model::SecuritySchemeData {
    fn to_cli(self) -> golem_cli::model::ApiSecurityScheme {
        golem_cli::model::ApiSecurityScheme {
            scheme_identifier: self.scheme_identifier,
            client_id: self.client_id,
            client_secret: self.client_secret,
            redirect_url: self.redirect_url,
            scopes: self.scopes,
        }
    }
}

impl ToCli<Component> for golem_cloud_client::model::Component {
    fn to_cli(self) -> Component {
        Component {
            versioned_component_id: self.versioned_component_id.to_oss(),
            component_name: self.component_name,
            component_size: self.component_size,
            metadata: self.metadata,
            project_id: Some(golem_cli::cloud::ProjectId(self.project_id)),
            created_at: self.created_at,
            component_type: self
                .component_type
                .unwrap_or(golem_client::model::ComponentType::Durable),
            files: self.files,
        }
    }
}
