use crate::gateway_binding::WorkerDetail;
use crate::service::worker::{WorkerService, WorkerServiceError};
use crate::empty_worker_metadata;
use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use futures_util::TryStreamExt;
use golem_common::model::{HasAccountId, TargetWorkerId};
use http::StatusCode;
use poem::web::headers::ContentType;
use rib::{RibResult, LiteralValue};
use std::pin::Pin;
use std::sync::Arc;
use serde_json::Value;
use mime_guess::mime::Mime;

#[async_trait]
pub trait SwaggerUiBindingHandler<Namespace> {
    async fn handle_swagger_ui_binding_result(
        &self,
        namespace: &Namespace,
        worker_detail: &WorkerDetail,
        original_result: RibResult,
    ) -> SwaggerUiBindingResult;
}

pub type SwaggerUiBindingResult = Result<SwaggerUiBindingSuccess, SwaggerUiBindingError>;

pub struct SwaggerUiBindingSuccess {
    pub binding_details: SwaggerUiBindingDetails,
    pub data: Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static>>,
}

pub enum SwaggerUiBindingError {
    InternalError(String),
    InvalidRibResult(String),
    WorkerServiceError(WorkerServiceError),
}

#[derive(Debug, Clone)]
pub struct SwaggerUiBindingDetails {
    pub content_type: ContentType,
    pub status_code: StatusCode,
    pub mount_path: String,
}

pub struct DefaultSwaggerUiBindingHandler {
    worker_service: Arc<dyn WorkerService + Sync + Send>,
}

impl DefaultSwaggerUiBindingHandler {
    pub fn new(worker_service: Arc<dyn WorkerService + Sync + Send>) -> Self {
        DefaultSwaggerUiBindingHandler { worker_service }
    }
}

#[async_trait]
impl<Namespace: HasAccountId + Send + Sync + 'static> SwaggerUiBindingHandler<Namespace>
    for DefaultSwaggerUiBindingHandler
{
    async fn handle_swagger_ui_binding_result(
        &self,
        _namespace: &Namespace,
        worker_detail: &WorkerDetail,
        original_result: RibResult,
    ) -> SwaggerUiBindingResult {
        let binding_details = SwaggerUiBindingDetails::from_rib_result(original_result)
            .map_err(SwaggerUiBindingError::InvalidRibResult)?;

        let worker_id = TargetWorkerId {
            component_id: worker_detail.component_id.component_id.clone(),
            worker_name: worker_detail.worker_name.clone(),
        };

        let stream = self
            .worker_service
            .get_swagger_ui_contents(&worker_id, binding_details.mount_path.clone(), empty_worker_metadata())
            .await
            .map_err(SwaggerUiBindingError::WorkerServiceError)?;

        let stream = stream.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()));

        Ok(SwaggerUiBindingSuccess {
            binding_details,
            data: Box::pin(stream),
        })
    }
}

impl SwaggerUiBindingDetails {
    pub fn from_rib_result(result: RibResult) -> Result<SwaggerUiBindingDetails, String> {
        // Expected format:
        // {
        //   "mount_path": "/swagger-ui",
        //   "content_type": "text/html",
        //   "status_code": 200
        // }
        let value = result.get_literal().ok_or("Expected literal value")?;
        let json_value = match value {
            LiteralValue::String(s) => serde_json::from_str::<Value>(&s)
                .map_err(|e| format!("Failed to parse JSON string: {}", e))?,
            _ => return Err("Expected JSON string literal".to_string()),
        };
        
        let obj = json_value.as_object().ok_or("Expected object")?;

        let mount_path = obj
            .get("mount_path")
            .and_then(|v| v.as_str())
            .ok_or("Missing mount_path")?
            .to_string();

        let content_type = obj
            .get("content_type")
            .and_then(|v| v.as_str())
            .ok_or("Missing content_type")?;
        let mime: Mime = content_type.parse().map_err(|_| "Invalid content_type".to_string())?;
        let content_type = ContentType::from(mime);

        let status_code = obj
            .get("status_code")
            .and_then(|v| v.as_u64())
            .ok_or("Missing status_code")?;
        let status_code = StatusCode::from_u16(status_code as u16)
            .map_err(|_| "Invalid status_code".to_string())?;

        Ok(SwaggerUiBindingDetails {
            content_type,
            status_code,
            mount_path,
        })
    }
} 