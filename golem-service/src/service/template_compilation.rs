use async_trait::async_trait;
use golem_api_grpc::proto::golem::templatecompilation::{
    template_compilation_service_client::TemplateCompilationServiceClient,
    TemplateCompilationRequest,
};
use golem_common::model::TemplateId;

#[async_trait]
pub trait TemplateCompilationService {
    async fn enqueue_compilation(&self, template_id: &TemplateId, template_version: i32);
}

pub struct TemplateCompilationServiceDefault {
    uri: http_02::Uri,
}

impl TemplateCompilationServiceDefault {
    pub fn new(host: String, port: u16) -> Self {
        let uri = format!("http://{}:{}", host, port)
            .parse()
            .expect("Failed to parse TemplateCompilationService URI");
        Self { uri }
    }
}

#[async_trait]
impl TemplateCompilationService for TemplateCompilationServiceDefault {
    async fn enqueue_compilation(&self, template_id: &TemplateId, template_version: i32) {
        let mut client = match TemplateCompilationServiceClient::connect(self.uri.clone()).await {
            Ok(client) => client,
            Err(e) => {
                tracing::error!("Failed to connect to TemplateCompilationService: {e:?}");
                return;
            }
        };

        let request = TemplateCompilationRequest {
            template_id: Some(template_id.clone().into()),
            template_version,
        };

        match client.enqueue_compilation(request).await {
            Ok(_) => tracing::info!(
                "Enqueued compilation for template {template_id} version {template_version}",
            ),
            Err(e) => tracing::error!("Failed to enqueue compilation: {e:?}"),
        }
    }
}

pub struct TemplateCompilationServiceDisabled;

#[async_trait]
impl TemplateCompilationService for TemplateCompilationServiceDisabled {
    async fn enqueue_compilation(&self, _: &TemplateId, _: i32) {}
}
