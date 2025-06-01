use axum::body::Bytes;
use axum::extract::{FromRequest, Request};
use axum::http::HeaderMap;
use axum_jrpc::error::{JsonRpcError, JsonRpcErrorReason};
use axum_jrpc::{JsonRpcExtractor, JsonRpcResponse};
use cloud_common::auth::GolemSecurityScheme;
use serde_json::Value;

// A wrapper over JsonRpcExtractor to deal with extra authentication
pub struct DebugServiceRequest {
    pub json_rpc_extractor: JsonRpcExtractor,
    pub security_scheme: GolemSecurityScheme,
}

#[async_trait::async_trait]
impl<S> FromRequest<S> for DebugServiceRequest
where
    Bytes: FromRequest<S>,
    S: Send + Sync,
{
    type Rejection = JsonRpcResponse;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let headers: &HeaderMap = req.headers();
        let security_scheme = GolemSecurityScheme::from_header_map(headers);
        let json_rpc_extractor = JsonRpcExtractor::from_request(req, state).await?;

        match security_scheme {
            Ok(security_scheme) => Ok(DebugServiceRequest {
                json_rpc_extractor,
                security_scheme,
            }),
            Err(e) => Err(JsonRpcResponse::error(
                json_rpc_extractor.get_answer_id(),
                JsonRpcError::new(
                    JsonRpcErrorReason::ServerError(-32099),
                    e.to_string(),
                    Value::default(),
                ),
            )),
        }
    }
}
