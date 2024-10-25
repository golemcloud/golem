use std::sync::Arc;
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use tokio_stream::StreamExt;
use tonic::{Request, Response, Status};
use tracing::Instrument;
use golem_api_grpc::proto::golem::common::{ErrorBody, ErrorsBody};
use golem_api_grpc::proto::golem::component::v1::{component_error, ComponentError, DownloadComponentResponse, DownloadIfsRequest, DownloadIfsResponse};
use golem_api_grpc::proto::golem::component::v1::download_ifs_response::Result::{Error, SuccessChunk};
use golem_component_service_base::service::ifs::InitialFileSystemService;
use golem_service_base::auth::DefaultNamespace;
use golem_api_grpc::proto::golem::component::v1::ifs_service_server::IfsService;
use golem_common::grpc::proto_component_id_string;
use golem_common::recorded_grpc_api_request;
use golem_component_service_base::api::common::ComponentTraceErrorKind;
use golem_service_base::stream::ByteStream;

pub struct IFSGrpcApi{
    pub ifs_service: Arc<dyn InitialFileSystemService<DefaultNamespace> + Send + Sync>
}

impl IFSGrpcApi {
    async fn download(
        &self,
        request: DownloadIfsRequest,
    ) -> Result<ByteStream, ComponentError> {
        let id = request.component_id.and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing component_id"))?;

        let version = request.version;
        let result = self
            .ifs_service
            .download_stream(&id, version, &Default::default()).await?;
        Ok(result)

    }

}

fn bad_request_error(error: &str) -> ComponentError {
    ComponentError {
        error: Some(component_error::Error::BadRequest(ErrorsBody {
            errors: vec![error.to_string()],
        })),
    }
}

#[async_trait::async_trait]
impl IfsService for IFSGrpcApi {
    type DownloadIFSStream = BoxStream<'static, Result<DownloadIfsResponse, Status>>;

    async fn download_ifs(&self, request: Request<DownloadIfsRequest>) -> Result<Response<Self::DownloadIFSStream>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "download_component",
            component_id = proto_component_id_string(&request.component_id)
        );
        let stream: Self::DownloadIFSStream   = match self.download(request).instrument(record.span.clone()).await{
            Ok(response) => {
                let stream = response.map(|content| {
                    let res = match content {
                        Ok(content) => DownloadIfsResponse{
                            result: Some(SuccessChunk(content)),
                        },
                        Err(_) => DownloadIfsResponse{
                            result: Some(Error(
                                internal_error("Internal error")
                            ))
                        }
                    };
                    Ok(res)
                });
                let stream: Self::DownloadIFSStream = Box::pin(stream);
                record.succeed(stream)
            }
            Err(err) => {
                let res = DownloadIfsResponse{
                    result: Some(Error(err.clone())),
                };
                let stream  = Box::pin(tokio_stream::iter([Ok(res)]));
                record.fail(stream, &ComponentTraceErrorKind(&err))
            }
        };
        Ok(Response::new(stream))
    }
}

fn internal_error(error: &str) -> ComponentError {
    ComponentError {
        error: Some(component_error::Error::InternalError(ErrorBody {
            error: error.to_string(),
        })),
    }
}