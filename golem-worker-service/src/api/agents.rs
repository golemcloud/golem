use crate::api::common::ApiEndpointError;
use crate::service::auth::{AuthService, AuthServiceError};
use crate::service::worker::{
    WorkerService, WorkerServiceError, validate_public_session_schema_value,
};
use chrono::{DateTime, Utc};
use futures::{SinkExt, StreamExt};
use golem_api_grpc::invocation_session_protocol::InvocationSessionState;
use golem_api_grpc::proto::golem::worker::{
    InvocationRejected, InvocationRejectionReason, InvocationRequest, InvocationResponse,
    PublicInvocationRequest, PublicInvocationStart, input_stream_item, invocation_request,
    invocation_response, public_invocation_request,
};
use golem_common::base_model::api;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::application::ApplicationName;
use golem_common::model::component::ComponentRevision;
use golem_common::model::environment::EnvironmentName;
use golem_common::model::worker::AgentConfigEntryDto;
use golem_common::model::{AgentId, IdempotencyKey};
use golem_common::schema::{SchemaValue, TypedSchemaValue};
use golem_common::{SafeDisplay, recorded_http_api_request};
use golem_service_base::api_tags::ApiTags;
use golem_service_base::clients::registry::RegistryServiceError;
use golem_service_base::model::auth::{AuthCtx, GolemSecurityScheme};
use poem::web::websocket::{
    BoxWebSocketUpgraded, CloseCode, Message, WebSocket, WebSocketConfig, WebSocketStream,
};
use poem_openapi::param::Header;
use poem_openapi::payload::Json;
use poem_openapi_derive::{Enum, Object, OpenApi};
use prost::Message as ProstMessage;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::Instrument;
use uuid::Uuid;

type Result<T> = std::result::Result<T, ApiEndpointError>;

const INVOCATION_SESSION_CHANNEL_CAPACITY: usize = 16;
const INVOCATION_SESSION_MAX_MESSAGE_SIZE: usize = 32 * 1024 * 1024;
const INVOCATION_SESSION_STAGING_ADMISSION_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(30);
const INVOCATION_SESSION_WRITE_PROGRESS_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(30);

pub struct AgentsApi {
    worker_service: Arc<WorkerService>,
    auth_service: Arc<dyn AuthService>,
}

#[OpenApi(prefix_path = "/v1/agents", tag = ApiTags::Agent)]
impl AgentsApi {
    pub fn new(worker_service: Arc<WorkerService>, auth_service: Arc<dyn AuthService>) -> Self {
        Self {
            worker_service,
            auth_service,
        }
    }

    #[oai(path = "/invoke-agent", method = "post", operation_id = "invoke_agent")]
    async fn invoke_agent(
        &self,
        mut request: Json<AgentInvocationRequest>,
        #[oai(name = "Idempotency-Key")] idempotency_key: Header<Option<IdempotencyKey>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<AgentInvocationResult>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        if request.idempotency_key.is_none() {
            request.idempotency_key = idempotency_key.0;
        }

        if let Some(ref mut email) = request.owner_account_email {
            let trimmed = email.trim().to_string();
            if trimmed.is_empty() {
                return Err(ApiEndpointError::bad_request(
                    api::error_code::VALIDATION_ERROR,
                    golem_common::safe("owner_account_email cannot be empty".to_string()),
                ));
            }
            *email = trimmed;
        }

        let record = recorded_http_api_request!(
            "invoke_agent",
            app = %request.app_name,
            env = %request.env_name,
            agent_type = %request.agent_type_name,
            idempotency_key = request.idempotency_key.as_ref().as_ref().map(|v| v.value.clone()),
            method = %request.method_name
        );

        let response = self
            .worker_service
            .invoke_agent_rest(request.0, auth)
            .instrument(record.span.clone())
            .await
            .map_err(Into::into);

        record.result(response).map(Json)
    }

    /// Invoke an agent through an attached live streaming session
    #[oai(
        path = "/invoke-agent-session",
        method = "get",
        operation_id = "invoke_agent_session"
    )]
    async fn invoke_agent_session(
        &self,
        websocket: WebSocket,
        token: GolemSecurityScheme,
    ) -> Result<BoxWebSocketUpgraded> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let worker_service = self.worker_service.clone();

        Ok(websocket
            .config(invocation_session_websocket_config())
            .on_upgrade(Box::new(move |socket| {
                Box::pin(proxy_public_invocation_session(
                    socket,
                    worker_service,
                    auth,
                ))
            })))
    }

    #[oai(path = "/create-agent", method = "post", operation_id = "create_agent")]
    async fn create_agent(
        &self,
        request: Json<CreateAgentRequest>,
        token: GolemSecurityScheme,
    ) -> Result<Json<CreateAgentResponse>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let record = recorded_http_api_request!(
            "create_agent",
            app = %request.app_name,
            env = %request.env_name,
            agent_type = %request.agent_type_name,
        );

        let response = self
            .worker_service
            .create_agent_rest(request.0, auth)
            .instrument(record.span.clone())
            .await
            .map_err(Into::into);

        record.result(response).map(Json)
    }
}

fn invocation_session_websocket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(INVOCATION_SESSION_MAX_MESSAGE_SIZE))
        .max_frame_size(Some(INVOCATION_SESSION_MAX_MESSAGE_SIZE))
        .max_write_buffer_size(2 * INVOCATION_SESSION_MAX_MESSAGE_SIZE)
}

#[derive(Debug)]
enum PublicSessionMessage {
    Request(PublicInvocationRequest),
    Ping(Vec<u8>),
    Pong,
    Close,
}

enum InitialPublicInvocation {
    Start(
        PublicInvocationStart,
        Option<golem_api_grpc::proto::golem::worker::IdempotencyKey>,
    ),
    Closed,
    WriterStopped,
}

#[derive(Debug)]
struct PublicSessionFrameError {
    close_code: CloseCode,
    reason: String,
}

fn decode_public_session_message(
    message: Message,
) -> std::result::Result<PublicSessionMessage, PublicSessionFrameError> {
    match message {
        Message::Binary(bytes) => PublicInvocationRequest::decode(bytes.as_slice())
            .map(PublicSessionMessage::Request)
            .map_err(|error| PublicSessionFrameError {
                close_code: CloseCode::Protocol,
                reason: format!("malformed invocation request: {error}"),
            }),
        Message::Text(_) => Err(PublicSessionFrameError {
            close_code: CloseCode::Unsupported,
            reason: "invocation sessions accept binary protobuf messages only".to_string(),
        }),
        Message::Ping(payload) => Ok(PublicSessionMessage::Ping(payload)),
        Message::Pong(_) => Ok(PublicSessionMessage::Pong),
        Message::Close(_) => Ok(PublicSessionMessage::Close),
    }
}

async fn proxy_public_invocation_session(
    socket: WebSocketStream,
    worker_service: Arc<WorkerService>,
    auth: AuthCtx,
) {
    let (websocket_sink, mut websocket_stream) = socket.split();
    let (websocket_sender, websocket_receiver) =
        tokio::sync::mpsc::channel(INVOCATION_SESSION_CHANNEL_CAPACITY);
    let mut writer = tokio::spawn(forward_websocket_messages(
        websocket_sink,
        websocket_receiver,
        INVOCATION_SESSION_WRITE_PROGRESS_TIMEOUT,
    ));
    let mut state = InvocationSessionState::default();

    let (start, idempotency_key) = match receive_public_invocation_start_while_writing(
        &mut websocket_stream,
        &websocket_sender,
        &mut state,
        &mut writer,
    )
    .await
    {
        InitialPublicInvocation::Start(start, idempotency_key) => (start, idempotency_key),
        InitialPublicInvocation::Closed => {
            drop(websocket_sender);
            let _ = writer.await;
            return;
        }
        InitialPublicInvocation::WriterStopped => return,
    };

    let (request_sender, request_receiver) =
        tokio::sync::mpsc::channel(INVOCATION_SESSION_CHANNEL_CAPACITY);
    let tail = tokio_stream::wrappers::ReceiverStream::new(request_receiver);
    let responses = match worker_service
        .invoke_public_agent_session(start, Box::pin(tail), auth)
        .await
    {
        Ok(responses) => responses,
        Err(error) => {
            let response = rejection_response(
                rejection_reason(&error),
                error.to_safe_string(),
                idempotency_key,
            );
            if state.validate_response(&response).is_ok()
                && queue_invocation_response(&websocket_sender, response).await
            {
                queue_websocket_close(&websocket_sender, CloseCode::Normal, "session rejected")
                    .await;
            }
            drop(websocket_sender);
            let _ = writer.await;
            return;
        }
    };

    let state = Arc::new(tokio::sync::Mutex::new(state));
    let requests = tokio::spawn(forward_public_requests(
        websocket_stream,
        request_sender,
        websocket_sender.clone(),
        state.clone(),
    ));
    let responses = tokio::spawn(forward_internal_responses(
        responses,
        websocket_sender.clone(),
        state,
    ));
    drop(websocket_sender);

    supervise_public_invocation_session(requests, responses, writer).await;
}

async fn receive_public_invocation_start_while_writing<S>(
    websocket_stream: &mut S,
    websocket_sender: &tokio::sync::mpsc::Sender<Message>,
    state: &mut InvocationSessionState,
    writer: &mut tokio::task::JoinHandle<()>,
) -> InitialPublicInvocation
where
    S: futures::Stream<Item = std::io::Result<Message>> + Unpin,
{
    let receive = receive_public_invocation_start(websocket_stream, websocket_sender, state);
    tokio::pin!(receive);
    tokio::select! {
        result = &mut receive => match result {
            Some((start, idempotency_key)) => {
                InitialPublicInvocation::Start(start, idempotency_key)
            }
            None => InitialPublicInvocation::Closed,
        },
        _ = writer => InitialPublicInvocation::WriterStopped,
    }
}

async fn receive_public_invocation_start<S>(
    websocket_stream: &mut S,
    websocket_sender: &tokio::sync::mpsc::Sender<Message>,
    state: &mut InvocationSessionState,
) -> Option<(
    PublicInvocationStart,
    Option<golem_api_grpc::proto::golem::worker::IdempotencyKey>,
)>
where
    S: futures::Stream<Item = std::io::Result<Message>> + Unpin,
{
    let first_request = loop {
        match websocket_stream.next().await {
            Some(Ok(message)) => match decode_public_session_message(message) {
                Ok(PublicSessionMessage::Request(request)) => break request,
                Ok(PublicSessionMessage::Ping(payload)) => {
                    if matches!(
                        websocket_sender.try_send(Message::pong(payload)),
                        Err(tokio::sync::mpsc::error::TrySendError::Closed(_))
                    ) {
                        return None;
                    }
                }
                Ok(PublicSessionMessage::Pong) => {}
                Ok(PublicSessionMessage::Close) => return None,
                Err(error) => {
                    queue_websocket_close(websocket_sender, error.close_code, error.reason).await;
                    return None;
                }
            },
            None | Some(Err(_)) => return None,
        }
    };

    if let Err(error) = state.validate_public_request(&first_request) {
        queue_websocket_close(websocket_sender, CloseCode::Protocol, error).await;
        return None;
    }

    match first_request.request {
        Some(public_invocation_request::Request::Start(start)) => {
            let idempotency_key = start.idempotency_key.clone();
            Some((start, idempotency_key))
        }
        Some(public_invocation_request::Request::ResumeAttach(resume)) => {
            let response = rejection_response(
                InvocationRejectionReason::ResumeUnsupported,
                "resume-attach is not supported by provisional live sessions".to_string(),
                resume.idempotency_key,
            );
            if state.validate_response(&response).is_ok()
                && queue_invocation_response(websocket_sender, response).await
            {
                queue_websocket_close(websocket_sender, CloseCode::Normal, "session rejected")
                    .await;
            }
            None
        }
        _ => {
            queue_websocket_close(
                websocket_sender,
                CloseCode::Protocol,
                "the first invocation request must be start or resume-attach",
            )
            .await;
            None
        }
    }
}

async fn forward_websocket_messages<S>(
    mut websocket_sink: S,
    mut websocket_receiver: tokio::sync::mpsc::Receiver<Message>,
    write_progress_timeout: std::time::Duration,
) where
    S: futures::Sink<Message> + Unpin,
{
    while let Some(message) = websocket_receiver.recv().await {
        let close = matches!(message, Message::Close(_));
        if !matches!(
            tokio::time::timeout(write_progress_timeout, websocket_sink.send(message)).await,
            Ok(Ok(()))
        ) {
            return;
        }
        if close {
            return;
        }
    }
}

async fn supervise_public_invocation_session(
    mut requests: tokio::task::JoinHandle<()>,
    mut responses: tokio::task::JoinHandle<()>,
    mut writer: tokio::task::JoinHandle<()>,
) {
    enum CompletedPump {
        Requests,
        Responses,
        Writer,
    }
    let completed = tokio::select! {
        _ = &mut requests => CompletedPump::Requests,
        _ = &mut responses => CompletedPump::Responses,
        _ = &mut writer => CompletedPump::Writer,
    };
    match completed {
        CompletedPump::Requests => {
            responses.abort();
            let _ = responses.await;
            let _ = writer.await;
        }
        CompletedPump::Responses => {
            requests.abort();
            let _ = requests.await;
            let _ = writer.await;
        }
        CompletedPump::Writer => {
            requests.abort();
            responses.abort();
            let _ = requests.await;
            let _ = responses.await;
        }
    }
}

async fn forward_public_requests<S>(
    websocket_stream: S,
    request_sender: tokio::sync::mpsc::Sender<InvocationRequest>,
    websocket_sender: tokio::sync::mpsc::Sender<Message>,
    state: Arc<tokio::sync::Mutex<InvocationSessionState>>,
) where
    S: futures::Stream<Item = std::io::Result<Message>> + Unpin,
{
    forward_public_requests_with_timeout(
        websocket_stream,
        request_sender,
        websocket_sender,
        state,
        INVOCATION_SESSION_STAGING_ADMISSION_TIMEOUT,
    )
    .await;
}

async fn forward_public_requests_with_timeout<S>(
    websocket_stream: S,
    request_sender: tokio::sync::mpsc::Sender<InvocationRequest>,
    websocket_sender: tokio::sync::mpsc::Sender<Message>,
    state: Arc<tokio::sync::Mutex<InvocationSessionState>>,
    staging_admission_timeout: std::time::Duration,
) where
    S: futures::Stream<Item = std::io::Result<Message>> + Unpin,
{
    let (staging_sender, mut staging_receiver) =
        tokio::sync::mpsc::channel(INVOCATION_SESSION_CHANNEL_CAPACITY);
    let read_requests = read_public_requests(
        websocket_stream,
        staging_sender,
        websocket_sender.clone(),
        state,
        staging_admission_timeout,
    );
    let forward_requests = async move {
        while let Some(request) = staging_receiver.recv().await {
            if request_sender.send(request).await.is_err() {
                try_queue_websocket_close(
                    &websocket_sender,
                    CloseCode::Error,
                    "internal invocation request stream closed",
                );
                return;
            }
        }
    };
    tokio::pin!(read_requests, forward_requests);
    tokio::select! {
        _ = &mut read_requests => {}
        _ = &mut forward_requests => {}
    }
}

async fn read_public_requests<S>(
    mut websocket_stream: S,
    staging_sender: tokio::sync::mpsc::Sender<InvocationRequest>,
    websocket_sender: tokio::sync::mpsc::Sender<Message>,
    state: Arc<tokio::sync::Mutex<InvocationSessionState>>,
    staging_admission_timeout: std::time::Duration,
) where
    S: futures::Stream<Item = std::io::Result<Message>> + Unpin,
{
    let mut pending_message = None;
    loop {
        let message = match pending_message.take() {
            Some(message) => message,
            None => match websocket_stream.next().await {
                Some(message) => message,
                None => return,
            },
        };
        let message = match message {
            Ok(message) => message,
            Err(_) => return,
        };
        let request = match decode_public_session_message(message) {
            Ok(PublicSessionMessage::Request(request)) => request,
            Ok(PublicSessionMessage::Ping(payload)) => {
                if matches!(
                    websocket_sender.try_send(Message::pong(payload)),
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_))
                ) {
                    return;
                }
                continue;
            }
            Ok(PublicSessionMessage::Pong) => continue,
            Ok(PublicSessionMessage::Close) => return,
            Err(error) => {
                try_queue_websocket_close(&websocket_sender, error.close_code, error.reason);
                return;
            }
        };
        let validation = {
            let mut state = state.lock().await;
            state.validate_public_request(&request)
        };
        if let Err(error) = validation {
            try_queue_websocket_close(&websocket_sender, CloseCode::Protocol, error);
            return;
        }
        let request = match trusted_tail_request(request) {
            Ok(request) => request,
            Err(error) => {
                try_queue_websocket_close(&websocket_sender, CloseCode::Protocol, error);
                return;
            }
        };
        let admission =
            tokio::time::timeout(staging_admission_timeout, staging_sender.send(request));
        tokio::pin!(admission);
        tokio::select! {
            result = &mut admission => {
                if !matches!(result, Ok(Ok(()))) {
                    return;
                }
            }
            next = websocket_stream.next() => {
                match next {
                    None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                    Some(message) => pending_message = Some(message),
                }
                if !matches!(admission.await, Ok(Ok(()))) {
                    return;
                }
            }
        }
    }
}

async fn forward_internal_responses<S>(
    mut responses: S,
    websocket_sender: tokio::sync::mpsc::Sender<Message>,
    state: Arc<tokio::sync::Mutex<InvocationSessionState>>,
) where
    S: futures::Stream<Item = std::result::Result<InvocationResponse, tonic::Status>> + Unpin,
{
    while let Some(response) = responses.next().await {
        let response = match response {
            Ok(response) => response,
            Err(_) => break,
        };
        let validation = {
            let mut state = state.lock().await;
            state
                .validate_response(&response)
                .map(|()| state.is_complete())
        };
        let complete = match validation {
            Ok(complete) => complete,
            Err(error) => {
                queue_websocket_close(&websocket_sender, CloseCode::Protocol, error).await;
                return;
            }
        };
        if websocket_sender
            .send(Message::binary(response.encode_to_vec()))
            .await
            .is_err()
        {
            return;
        }
        if complete {
            queue_websocket_close(&websocket_sender, CloseCode::Normal, "session complete").await;
            return;
        }
    }

    queue_websocket_close(
        &websocket_sender,
        CloseCode::Error,
        "internal invocation response stream ended before session completion",
    )
    .await;
}

async fn queue_websocket_close(
    sender: &tokio::sync::mpsc::Sender<Message>,
    code: CloseCode,
    reason: impl AsRef<str>,
) {
    let reason = bounded_close_reason(reason.as_ref());
    let _ = sender.send(Message::close_with(code, reason)).await;
}

fn try_queue_websocket_close(
    sender: &tokio::sync::mpsc::Sender<Message>,
    code: CloseCode,
    reason: impl AsRef<str>,
) {
    let reason = bounded_close_reason(reason.as_ref());
    let _ = sender.try_send(Message::close_with(code, reason));
}

fn trusted_tail_request(
    request: PublicInvocationRequest,
) -> std::result::Result<InvocationRequest, String> {
    let request = match request.request {
        Some(public_invocation_request::Request::InputItem(item)) => {
            if let Some(input_stream_item::Payload::Value(value)) = &item.payload {
                validate_public_session_schema_value(value)?;
            }
            invocation_request::Request::InputItem(item)
        }
        Some(public_invocation_request::Request::InputEnd(end)) => {
            invocation_request::Request::InputEnd(end)
        }
        Some(public_invocation_request::Request::StreamCancel(cancel)) => {
            invocation_request::Request::StreamCancel(cancel)
        }
        Some(public_invocation_request::Request::Start(_))
        | Some(public_invocation_request::Request::ResumeAttach(_)) => {
            return Err(
                "invocation start or resume-attach may only appear as the first request"
                    .to_string(),
            );
        }
        None => return Err("invocation request has no payload".to_string()),
    };
    Ok(InvocationRequest {
        request: Some(request),
    })
}

fn rejection_reason(error: &WorkerServiceError) -> InvocationRejectionReason {
    match error {
        WorkerServiceError::TypeChecker(_)
        | WorkerServiceError::RegistryServiceError(RegistryServiceError::BadRequest(_)) => {
            InvocationRejectionReason::Validation
        }
        WorkerServiceError::AuthError(AuthServiceError::Unauthorized(_))
        | WorkerServiceError::RegistryServiceError(RegistryServiceError::Unauthorized(_))
        | WorkerServiceError::RegistryServiceError(RegistryServiceError::CouldNotAuthenticate(_)) => {
            InvocationRejectionReason::Unauthorized
        }
        WorkerServiceError::ComponentNotFound(_)
        | WorkerServiceError::AgentNotFound(_)
        | WorkerServiceError::RegistryServiceError(RegistryServiceError::NotFound(_)) => {
            InvocationRejectionReason::NotFound
        }
        _ => InvocationRejectionReason::Internal,
    }
}

fn rejection_response(
    reason: InvocationRejectionReason,
    error: String,
    idempotency_key: Option<golem_api_grpc::proto::golem::worker::IdempotencyKey>,
) -> InvocationResponse {
    InvocationResponse {
        response: Some(invocation_response::Response::Rejected(
            InvocationRejected {
                reason: reason as i32,
                error,
                idempotency_key,
                agent_id: None,
                component_revision: None,
            },
        )),
    }
}

async fn queue_invocation_response(
    sender: &tokio::sync::mpsc::Sender<Message>,
    response: InvocationResponse,
) -> bool {
    sender
        .send(Message::binary(response.encode_to_vec()))
        .await
        .is_ok()
}

fn bounded_close_reason(reason: &str) -> String {
    const MAX_CLOSE_REASON_BYTES: usize = 123;
    if reason.len() <= MAX_CLOSE_REASON_BYTES {
        return reason.to_string();
    }
    let mut end = MAX_CLOSE_REASON_BYTES;
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    reason[..end].to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Enum)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub enum AgentInvocationMode {
    Await,
    Schedule,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct AgentInvocationRequest {
    pub app_name: ApplicationName,
    pub env_name: EnvironmentName,
    pub agent_type_name: AgentTypeName,
    pub parameters: SchemaValue,
    pub phantom_id: Option<Uuid>,
    #[oai(default)]
    #[serde(default)]
    pub config: Vec<AgentConfigEntryDto>,
    pub method_name: String,
    pub method_parameters: SchemaValue,
    pub mode: AgentInvocationMode,
    pub schedule_at: Option<DateTime<Utc>>,
    pub idempotency_key: Option<IdempotencyKey>,
    pub deployment_revision: Option<i64>,
    pub owner_account_email: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct AgentInvocationResult {
    pub agent_id: AgentId,
    pub idempotency_key: IdempotencyKey,
    pub result: Option<TypedSchemaValue>,
    pub component_revision: Option<ComponentRevision>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentRequest {
    pub app_name: ApplicationName,
    pub env_name: EnvironmentName,
    pub agent_type_name: AgentTypeName,
    pub parameters: SchemaValue,
    pub phantom_id: Option<Uuid>,
    #[oai(default)]
    #[serde(default)]
    pub config: Vec<AgentConfigEntryDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentResponse {
    pub agent_id: AgentId,
    pub component_revision: ComponentRevision,
}

#[cfg(test)]
mod tests {
    use super::{
        AgentInvocationRequest, CreateAgentRequest, INVOCATION_SESSION_MAX_MESSAGE_SIZE,
        InitialPublicInvocation, PublicSessionMessage, bounded_close_reason,
        decode_public_session_message, forward_internal_responses, forward_public_requests,
        forward_public_requests_with_timeout, forward_websocket_messages,
        invocation_session_websocket_config, queue_invocation_response,
        receive_public_invocation_start, receive_public_invocation_start_while_writing,
        rejection_reason, supervise_public_invocation_session, trusted_tail_request,
    };
    use crate::service::worker::WorkerServiceError;
    use futures::StreamExt;
    use golem_api_grpc::invocation_session_protocol::InvocationSessionState;
    use golem_api_grpc::proto::golem::worker::{
        AgentId, IdempotencyKey, InputStreamEnd, InputStreamItem, InvocationAccepted,
        InvocationRejectionReason, InvocationRequest, InvocationResponse,
        InvocationSessionCompletion, InvocationSessionResult, OutputStreamItem,
        PublicInvocationRequest, PublicInvocationStart, ResumeAttach, StreamCancel,
        StreamCancelReason, StreamCancelRole, input_stream_item, invocation_request,
        invocation_response, invocation_session_result, public_invocation_request,
    };
    use golem_service_base::clients::registry::RegistryServiceError;
    use poem::web::websocket::{CloseCode, Message};
    use poem_openapi::types::{ParseFromJSON, ToJSON};
    use prost::Message as ProstMessage;
    use serde_json::{Value, json};
    use std::sync::Arc;
    use test_r::test;

    fn empty_parameter_record() -> Value {
        json!({ "kind": "record", "value": { "fields": [] } })
    }

    fn gated_websocket_sink(
        permits: tokio::sync::mpsc::Receiver<()>,
        delivered: Arc<tokio::sync::Mutex<Vec<Message>>>,
    ) -> impl futures::Sink<Message, Error = ()> {
        futures::sink::unfold(
            (permits, delivered),
            |(mut permits, delivered), message| async move {
                permits.recv().await.ok_or(())?;
                delivered.lock().await.push(message);
                Ok((permits, delivered))
            },
        )
    }

    #[test]
    fn create_agent_request_preserves_canonical_schema_value_json() {
        let parameters = empty_parameter_record();
        let config_value = json!({ "arbitrary": [1, true, null] });
        let request_json = json!({
            "appName": "app",
            "envName": "env",
            "agentTypeName": "agent",
            "parameters": parameters,
            "config": [{
                "path": ["nested"],
                "value": config_value,
            }],
        });

        let request = CreateAgentRequest::parse_from_json(Some(request_json))
            .expect("canonical SchemaValue JSON must remain accepted");
        let encoded = request.to_json().expect("request must serialize to JSON");

        assert_eq!(encoded["parameters"], empty_parameter_record());
        assert_eq!(encoded["config"][0]["value"], config_value);
    }

    #[test]
    fn invoke_agent_request_preserves_canonical_schema_value_json() {
        let request_json = json!({
            "appName": "app",
            "envName": "env",
            "agentTypeName": "agent",
            "parameters": empty_parameter_record(),
            "methodName": "run",
            "methodParameters": empty_parameter_record(),
            "mode": "await",
        });

        let request = AgentInvocationRequest::parse_from_json(Some(request_json))
            .expect("canonical SchemaValue JSON must remain accepted");
        let encoded = request.to_json().expect("request must serialize to JSON");

        assert_eq!(encoded["parameters"], empty_parameter_record());
        assert_eq!(encoded["methodParameters"], empty_parameter_record());
    }

    #[test]
    fn agent_request_rejects_noncanonical_parameter_json() {
        let request_json = json!({
            "appName": "app",
            "envName": "env",
            "agentTypeName": "agent",
            "parameters": [],
        });

        assert!(CreateAgentRequest::parse_from_json(Some(request_json)).is_err());
    }

    #[test]
    fn invocation_session_frame_decoder_accepts_one_binary_protobuf_message() {
        let request = PublicInvocationRequest {
            request: Some(public_invocation_request::Request::InputEnd(
                InputStreamEnd {
                    stream_id: 7,
                    offset: 3,
                },
            )),
        };

        let decoded = decode_public_session_message(Message::binary(request.encode_to_vec()))
            .expect("binary protobuf frame must decode");

        assert!(matches!(
            decoded,
            PublicSessionMessage::Request(PublicInvocationRequest {
                request: Some(public_invocation_request::Request::InputEnd(
                    InputStreamEnd {
                        stream_id: 7,
                        offset: 3,
                    }
                )),
            })
        ));
    }

    #[test]
    fn invocation_session_frame_decoder_rejects_text_and_malformed_protobuf() {
        let text = decode_public_session_message(Message::text("not protobuf"))
            .expect_err("text frames must be rejected");
        assert_eq!(text.close_code, CloseCode::Unsupported);

        let malformed = decode_public_session_message(Message::binary([0xff, 0xff]))
            .expect_err("malformed protobuf must be rejected");
        assert_eq!(malformed.close_code, CloseCode::Protocol);
    }

    #[test]
    fn invocation_session_websocket_config_bounds_frames_messages_and_writes() {
        let config = invocation_session_websocket_config();

        assert_eq!(
            config.max_message_size,
            Some(INVOCATION_SESSION_MAX_MESSAGE_SIZE)
        );
        assert_eq!(
            config.max_frame_size,
            Some(INVOCATION_SESSION_MAX_MESSAGE_SIZE)
        );
        assert_eq!(
            config.max_write_buffer_size,
            2 * INVOCATION_SESSION_MAX_MESSAGE_SIZE
        );
    }

    #[test]
    fn public_tail_translation_cannot_construct_a_trusted_start() {
        let translated = trusted_tail_request(PublicInvocationRequest {
            request: Some(public_invocation_request::Request::InputEnd(
                InputStreamEnd {
                    stream_id: 4,
                    offset: 9,
                },
            )),
        })
        .unwrap();
        assert!(matches!(
            translated.request,
            Some(invocation_request::Request::InputEnd(InputStreamEnd {
                stream_id: 4,
                offset: 9,
            }))
        ));

        let repeated_start = PublicInvocationRequest {
            request: Some(public_invocation_request::Request::Start(Default::default())),
        };
        assert!(trusted_tail_request(repeated_start).is_err());
    }

    #[test]
    fn public_tail_translation_rejects_recursive_host_capabilities() {
        use golem_api_grpc::proto::golem::schema::{
            RecordValue, SchemaValue, SecretValue, schema_value,
        };

        let request = PublicInvocationRequest {
            request: Some(public_invocation_request::Request::InputItem(
                InputStreamItem {
                    stream_id: 7,
                    sequence: 0,
                    payload: Some(input_stream_item::Payload::Value(SchemaValue {
                        value: Some(schema_value::Value::RecordValue(RecordValue {
                            fields: vec![SchemaValue {
                                value: Some(schema_value::Value::SecretValue(
                                    SecretValue::default(),
                                )),
                            }],
                        })),
                    })),
                },
            )),
        };

        assert!(
            trusted_tail_request(request)
                .unwrap_err()
                .contains("host-managed capability")
        );
    }

    #[test]
    async fn saturated_websocket_output_does_not_hide_disconnect_after_ping() {
        let (websocket_client, websocket_stream) =
            futures::channel::mpsc::unbounded::<std::io::Result<Message>>();
        let (internal_sender, _internal_receiver) = tokio::sync::mpsc::channel(1);
        let (websocket_sender, _websocket_receiver) = tokio::sync::mpsc::channel(1);
        websocket_sender
            .send(Message::Ping(Vec::new()))
            .await
            .unwrap();

        let requests = tokio::spawn(forward_public_requests(
            websocket_stream,
            internal_sender,
            websocket_sender,
            Arc::new(tokio::sync::Mutex::new(InvocationSessionState::default())),
        ));
        websocket_client
            .unbounded_send(Ok(Message::Ping(Vec::new())))
            .unwrap();
        tokio::task::yield_now().await;
        drop(websocket_client);

        tokio::time::timeout(std::time::Duration::from_millis(100), requests)
            .await
            .expect("saturated WebSocket output hid the client disconnect")
            .unwrap();
    }

    #[test]
    async fn saturated_input_forwarding_does_not_block_responses_or_tail_cancellation() {
        use golem_api_grpc::proto::golem::schema::{
            RecordValue, SchemaValue, SchemaValueStreamReference, schema_value,
        };

        let idempotency_key = IdempotencyKey {
            value: "input-backpressure".to_string(),
        };
        let stream_id = 7;
        let start = PublicInvocationRequest {
            request: Some(public_invocation_request::Request::Start(
                PublicInvocationStart {
                    application_name: "app".to_string(),
                    environment_name: "env".to_string(),
                    agent_type_name: "agent".to_string(),
                    constructor_parameters: Some(SchemaValue {
                        value: Some(schema_value::Value::RecordValue(RecordValue::default())),
                    }),
                    method_name: "run".to_string(),
                    method_parameters: Some(SchemaValue {
                        value: Some(schema_value::Value::RecordValue(RecordValue {
                            fields: vec![SchemaValue {
                                value: Some(schema_value::Value::StreamReference(
                                    SchemaValueStreamReference { stream_id },
                                )),
                            }],
                        })),
                    }),
                    idempotency_key: Some(idempotency_key.clone()),
                    ..Default::default()
                },
            )),
        };
        let accepted = InvocationResponse {
            response: Some(invocation_response::Response::Accepted(
                InvocationAccepted {
                    agent_id: Some(AgentId {
                        component_id: None,
                        name: "agent".to_string(),
                    }),
                    idempotency_key: Some(idempotency_key),
                    component_revision: Some(1),
                },
            )),
        };
        let mut state = InvocationSessionState::default();
        state.validate_public_request(&start).unwrap();
        state.validate_response(&accepted).unwrap();
        let state = Arc::new(tokio::sync::Mutex::new(state));

        let (websocket_client, websocket_stream) =
            futures::channel::mpsc::unbounded::<std::io::Result<Message>>();
        let (internal_sender, mut internal_receiver) = tokio::sync::mpsc::channel(1);
        internal_sender
            .send(InvocationRequest { request: None })
            .await
            .unwrap();
        let (websocket_sender, mut websocket_receiver) = tokio::sync::mpsc::channel(2);
        let requests = tokio::spawn(forward_public_requests(
            websocket_stream,
            internal_sender,
            websocket_sender.clone(),
            state.clone(),
        ));
        let input = PublicInvocationRequest {
            request: Some(public_invocation_request::Request::InputItem(
                InputStreamItem {
                    stream_id,
                    sequence: 0,
                    payload: Some(input_stream_item::Payload::Value(
                        golem_common::schema::SchemaValue::U32(1)
                            .try_into()
                            .unwrap(),
                    )),
                },
            )),
        };
        websocket_client
            .unbounded_send(Ok(Message::binary(input.encode_to_vec())))
            .unwrap();
        tokio::task::yield_now().await;

        let result = InvocationResponse {
            response: Some(invocation_response::Response::Result(
                InvocationSessionResult {
                    result: Some(invocation_session_result::Result::NoResult(
                        golem_api_grpc::proto::golem::common::Empty {},
                    )),
                    component_revision: Some(1),
                    agent_id: Some(AgentId {
                        component_id: None,
                        name: "agent".to_string(),
                    }),
                    idempotency_key: Some(IdempotencyKey {
                        value: "input-backpressure".to_string(),
                    }),
                    ..Default::default()
                },
            )),
        };
        let responses = tokio::spawn(forward_internal_responses(
            tokio_stream::iter([Ok(result.clone())]),
            websocket_sender,
            state,
        ));
        let forwarded =
            tokio::time::timeout(std::time::Duration::from_secs(1), websocket_receiver.recv())
                .await
                .expect("response forwarding blocked behind saturated input")
                .expect("response pump closed unexpectedly");
        let Message::Binary(forwarded) = forwarded else {
            panic!("response pump did not forward a binary invocation response")
        };
        assert_eq!(
            InvocationResponse::decode(forwarded.as_slice()).unwrap(),
            result
        );

        drop(websocket_client);
        tokio::time::timeout(std::time::Duration::from_secs(1), requests)
            .await
            .expect("disconnect did not terminate the saturated request pump")
            .unwrap();
        let _ = responses.await;
        assert!(internal_receiver.recv().await.is_some());
        assert!(internal_receiver.recv().await.is_none());
    }

    #[test]
    async fn valid_input_burst_applies_backpressure_without_closing_the_session() {
        use golem_api_grpc::proto::golem::schema::{
            RecordValue, SchemaValue, SchemaValueStreamReference, schema_value,
        };

        let idempotency_key = IdempotencyKey {
            value: "lossless-input-backpressure".to_string(),
        };
        let stream_id = 7;
        let start = PublicInvocationRequest {
            request: Some(public_invocation_request::Request::Start(
                PublicInvocationStart {
                    application_name: "app".to_string(),
                    environment_name: "env".to_string(),
                    agent_type_name: "agent".to_string(),
                    constructor_parameters: Some(SchemaValue {
                        value: Some(schema_value::Value::RecordValue(RecordValue::default())),
                    }),
                    method_name: "run".to_string(),
                    method_parameters: Some(SchemaValue {
                        value: Some(schema_value::Value::StreamReference(
                            SchemaValueStreamReference { stream_id },
                        )),
                    }),
                    idempotency_key: Some(idempotency_key.clone()),
                    ..Default::default()
                },
            )),
        };
        let accepted = InvocationResponse {
            response: Some(invocation_response::Response::Accepted(
                InvocationAccepted {
                    agent_id: Some(AgentId {
                        component_id: None,
                        name: "agent".to_string(),
                    }),
                    idempotency_key: Some(idempotency_key),
                    component_revision: Some(1),
                },
            )),
        };
        let mut state = InvocationSessionState::default();
        state.validate_public_request(&start).unwrap();
        state.validate_response(&accepted).unwrap();

        let (websocket_client, websocket_stream) =
            futures::channel::mpsc::unbounded::<std::io::Result<Message>>();
        let (internal_sender, mut internal_receiver) = tokio::sync::mpsc::channel(1);
        internal_sender
            .send(InvocationRequest { request: None })
            .await
            .unwrap();
        let (websocket_sender, mut websocket_receiver) = tokio::sync::mpsc::channel(1);
        let requests = tokio::spawn(forward_public_requests(
            websocket_stream,
            internal_sender,
            websocket_sender,
            Arc::new(tokio::sync::Mutex::new(state)),
        ));

        for sequence in 0..18 {
            let input = PublicInvocationRequest {
                request: Some(public_invocation_request::Request::InputItem(
                    InputStreamItem {
                        stream_id,
                        sequence,
                        payload: Some(input_stream_item::Payload::Value(SchemaValue {
                            value: Some(schema_value::Value::U8Value(sequence as u32)),
                        })),
                    },
                )),
            };
            websocket_client
                .unbounded_send(Ok(Message::binary(input.encode_to_vec())))
                .unwrap();
        }

        let outbound = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            websocket_receiver.recv(),
        )
        .await;
        assert!(
            outbound.is_err(),
            "a valid input burst must be backpressured, not close the live session; got {outbound:?}"
        );
        assert!(!requests.is_finished());

        assert!(internal_receiver.recv().await.is_some());
        for expected_sequence in 0..18 {
            let request = tokio::time::timeout(
                std::time::Duration::from_secs(1),
                internal_receiver.recv(),
            )
            .await
            .expect("lossless input forwarding remained blocked after downstream capacity returned")
            .expect("internal invocation request stream closed during a valid input burst");
            assert!(matches!(
                request.request,
                Some(invocation_request::Request::InputItem(InputStreamItem {
                    stream_id: actual_stream_id,
                    sequence: actual_sequence,
                    ..
                })) if actual_stream_id == stream_id && actual_sequence == expected_sequence
            ));
        }
        drop(websocket_client);
        tokio::time::timeout(std::time::Duration::from_secs(1), requests)
            .await
            .expect("disconnect did not terminate the drained request pump")
            .unwrap();
        assert!(internal_receiver.recv().await.is_none());
    }

    #[test]
    async fn saturated_staging_does_not_hide_websocket_disconnect() {
        use golem_api_grpc::proto::golem::schema::{
            RecordValue, SchemaValue, SchemaValueStreamReference, schema_value,
        };

        let idempotency_key = IdempotencyKey {
            value: "saturated-staging-disconnect".to_string(),
        };
        let stream_id = 7;
        let start = PublicInvocationRequest {
            request: Some(public_invocation_request::Request::Start(
                PublicInvocationStart {
                    application_name: "app".to_string(),
                    environment_name: "env".to_string(),
                    agent_type_name: "agent".to_string(),
                    constructor_parameters: Some(SchemaValue {
                        value: Some(schema_value::Value::RecordValue(RecordValue::default())),
                    }),
                    method_name: "run".to_string(),
                    method_parameters: Some(SchemaValue {
                        value: Some(schema_value::Value::StreamReference(
                            SchemaValueStreamReference { stream_id },
                        )),
                    }),
                    idempotency_key: Some(idempotency_key.clone()),
                    ..Default::default()
                },
            )),
        };
        let accepted = InvocationResponse {
            response: Some(invocation_response::Response::Accepted(
                InvocationAccepted {
                    agent_id: Some(AgentId {
                        component_id: None,
                        name: "agent".to_string(),
                    }),
                    idempotency_key: Some(idempotency_key),
                    component_revision: Some(1),
                },
            )),
        };
        let mut state = InvocationSessionState::default();
        state.validate_public_request(&start).unwrap();
        state.validate_response(&accepted).unwrap();

        let (websocket_client, websocket_stream) =
            futures::channel::mpsc::unbounded::<std::io::Result<Message>>();
        let (internal_sender, mut internal_receiver) = tokio::sync::mpsc::channel(1);
        internal_sender
            .send(InvocationRequest { request: None })
            .await
            .unwrap();
        let (websocket_sender, _websocket_receiver) = tokio::sync::mpsc::channel(1);
        let requests = tokio::spawn(forward_public_requests_with_timeout(
            websocket_stream,
            internal_sender,
            websocket_sender,
            Arc::new(tokio::sync::Mutex::new(state)),
            std::time::Duration::from_millis(50),
        ));

        for sequence in 0..19 {
            let input = PublicInvocationRequest {
                request: Some(public_invocation_request::Request::InputItem(
                    InputStreamItem {
                        stream_id,
                        sequence,
                        payload: Some(input_stream_item::Payload::Value(SchemaValue {
                            value: Some(schema_value::Value::U8Value(sequence as u32)),
                        })),
                    },
                )),
            };
            websocket_client
                .unbounded_send(Ok(Message::binary(input.encode_to_vec())))
                .unwrap();
        }

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(!requests.is_finished());
        drop(websocket_client);

        tokio::time::timeout(std::time::Duration::from_millis(200), requests)
            .await
            .expect("saturated staging exceeded its bounded disconnect-detection deadline")
            .unwrap();
        assert!(internal_receiver.recv().await.is_some());
        assert!(internal_receiver.recv().await.is_none());
    }

    #[test]
    async fn saturated_output_forwarding_does_not_block_output_cancellation() {
        use golem_api_grpc::proto::golem::schema::{
            RecordValue, SchemaValue, SchemaValueStreamReference, schema_value,
        };

        let idempotency_key = IdempotencyKey {
            value: "output-backpressure".to_string(),
        };
        let start = PublicInvocationRequest {
            request: Some(public_invocation_request::Request::Start(
                PublicInvocationStart {
                    application_name: "app".to_string(),
                    environment_name: "env".to_string(),
                    agent_type_name: "agent".to_string(),
                    constructor_parameters: Some(SchemaValue {
                        value: Some(schema_value::Value::RecordValue(RecordValue::default())),
                    }),
                    method_name: "run".to_string(),
                    method_parameters: Some(SchemaValue {
                        value: Some(schema_value::Value::RecordValue(RecordValue::default())),
                    }),
                    idempotency_key: Some(idempotency_key.clone()),
                    ..Default::default()
                },
            )),
        };
        let agent_id = AgentId {
            component_id: None,
            name: "agent".to_string(),
        };
        let accepted = InvocationResponse {
            response: Some(invocation_response::Response::Accepted(
                InvocationAccepted {
                    agent_id: Some(agent_id.clone()),
                    idempotency_key: Some(idempotency_key.clone()),
                    component_revision: Some(1),
                },
            )),
        };
        let mut initial_state = InvocationSessionState::default();
        initial_state.validate_public_request(&start).unwrap();
        initial_state.validate_response(&accepted).unwrap();

        let output_stream_id = 2;
        let result = InvocationResponse {
            response: Some(invocation_response::Response::Result(
                InvocationSessionResult {
                    result: Some(invocation_session_result::Result::MethodResult(
                        SchemaValue {
                            value: Some(schema_value::Value::StreamReference(
                                SchemaValueStreamReference {
                                    stream_id: output_stream_id,
                                },
                            )),
                        },
                    )),
                    component_revision: Some(1),
                    agent_id: Some(agent_id),
                    idempotency_key: Some(idempotency_key),
                    ..Default::default()
                },
            )),
        };
        initial_state.validate_response(&result).unwrap();
        let state = Arc::new(tokio::sync::Mutex::new(initial_state));

        let output_item = InvocationResponse {
            response: Some(invocation_response::Response::OutputItem(
                OutputStreamItem {
                    stream_id: output_stream_id,
                    offset: 0,
                    value: Some(SchemaValue {
                        value: Some(schema_value::Value::U8Value(42)),
                    }),
                },
            )),
        };

        let (websocket_sender, mut websocket_receiver) = tokio::sync::mpsc::channel(1);
        websocket_sender
            .send(Message::Ping(Vec::new()))
            .await
            .unwrap();
        let responses = tokio::spawn(forward_internal_responses(
            tokio_stream::iter([Ok(output_item)]),
            websocket_sender.clone(),
            state.clone(),
        ));
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(
            !responses.is_finished(),
            "response forwarding did not saturate the bounded WebSocket queue"
        );
        assert!(
            state.try_lock().is_ok(),
            "response forwarding retained protocol state while blocked by output backpressure"
        );

        let (websocket_client, websocket_stream) =
            futures::channel::mpsc::unbounded::<std::io::Result<Message>>();
        let (internal_sender, mut internal_receiver) = tokio::sync::mpsc::channel(1);
        let requests = tokio::spawn(forward_public_requests(
            websocket_stream,
            internal_sender,
            websocket_sender,
            state,
        ));
        let cancel = PublicInvocationRequest {
            request: Some(public_invocation_request::Request::StreamCancel(
                StreamCancel {
                    stream_id: output_stream_id,
                    offset: 0,
                    role: StreamCancelRole::OutputConsumer as i32,
                    reason: StreamCancelReason::Cancelled as i32,
                    details: Some("stop output".to_string()),
                },
            )),
        };
        websocket_client
            .unbounded_send(Ok(Message::binary(cancel.encode_to_vec())))
            .unwrap();

        let forwarded = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            internal_receiver.recv(),
        )
        .await
        .expect("output cancellation was blocked behind saturated response forwarding")
        .expect("internal invocation request stream closed unexpectedly");
        assert!(matches!(
            forwarded.request,
            Some(invocation_request::Request::StreamCancel(StreamCancel {
                stream_id,
                role,
                ..
            })) if stream_id == output_stream_id && role == StreamCancelRole::OutputConsumer as i32
        ));

        drop(websocket_client);
        drop(websocket_receiver.recv().await);
        requests.abort();
        responses.abort();
    }

    #[test]
    async fn pre_start_rejection_waits_for_temporarily_backpressured_writer() {
        let request = PublicInvocationRequest {
            request: Some(public_invocation_request::Request::ResumeAttach(
                ResumeAttach {
                    idempotency_key: Some(IdempotencyKey {
                        value: "temporary-pre-start-stall".to_string(),
                    }),
                },
            )),
        };
        let mut websocket_stream =
            tokio_stream::iter([Ok(Message::binary(request.encode_to_vec()))]);
        let delivered = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let (permit_sender, permit_receiver) = tokio::sync::mpsc::channel(2);
        let sink = Box::pin(gated_websocket_sink(permit_receiver, delivered.clone()));
        let (websocket_sender, websocket_receiver) = tokio::sync::mpsc::channel(2);
        let writer = tokio::spawn(forward_websocket_messages(
            sink,
            websocket_receiver,
            std::time::Duration::from_secs(5),
        ));

        assert!(
            receive_public_invocation_start(
                &mut websocket_stream,
                &websocket_sender,
                &mut InvocationSessionState::default(),
            )
            .await
            .is_none()
        );
        drop(websocket_sender);
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        assert!(
            !writer.is_finished(),
            "temporary pre-start backpressure truncated the rejection"
        );

        permit_sender.send(()).await.unwrap();
        permit_sender.send(()).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), writer)
            .await
            .expect("pre-start writer did not drain after output progress resumed")
            .unwrap();

        let delivered = delivered.lock().await;
        assert_eq!(delivered.len(), 2);
        let Message::Binary(rejected) = &delivered[0] else {
            panic!("rejection was not delivered first")
        };
        assert!(matches!(
            InvocationResponse::decode(rejected.as_slice())
                .unwrap()
                .response,
            Some(invocation_response::Response::Rejected(_))
        ));
        assert!(matches!(delivered[1], Message::Close(_)));
    }

    #[test]
    async fn pre_start_ping_stall_terminates_when_the_writer_stops() {
        let messages = (0..18)
            .map(|_| Ok(Message::Ping(Vec::new())))
            .collect::<Vec<std::io::Result<Message>>>();
        let mut websocket_stream = tokio_stream::iter(messages).chain(futures::stream::pending());
        let delivered = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let (permit_sender, permit_receiver) = tokio::sync::mpsc::channel(1);
        let sink = Box::pin(gated_websocket_sink(permit_receiver, delivered.clone()));
        let (websocket_sender, websocket_receiver) = tokio::sync::mpsc::channel(1);
        let mut writer = tokio::spawn(forward_websocket_messages(
            sink,
            websocket_receiver,
            std::time::Duration::from_millis(50),
        ));

        let result = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            receive_public_invocation_start_while_writing(
                &mut websocket_stream,
                &websocket_sender,
                &mut InvocationSessionState::default(),
                &mut writer,
            ),
        )
        .await
        .expect("pre-start Ping stall outlived the writer progress deadline");
        assert!(matches!(result, InitialPublicInvocation::WriterStopped));
        assert!(delivered.lock().await.is_empty());
        drop(websocket_sender);
        drop(permit_sender);
    }

    #[test]
    async fn pre_start_rejection_terminates_at_writer_progress_deadline() {
        let request = PublicInvocationRequest {
            request: Some(public_invocation_request::Request::ResumeAttach(
                ResumeAttach {
                    idempotency_key: Some(IdempotencyKey {
                        value: "permanent-pre-start-stall".to_string(),
                    }),
                },
            )),
        };
        let mut websocket_stream =
            tokio_stream::iter([Ok(Message::binary(request.encode_to_vec()))]);
        let delivered = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let (permit_sender, permit_receiver) = tokio::sync::mpsc::channel(1);
        let sink = Box::pin(gated_websocket_sink(permit_receiver, delivered.clone()));
        let (websocket_sender, websocket_receiver) = tokio::sync::mpsc::channel(2);
        let writer = tokio::spawn(forward_websocket_messages(
            sink,
            websocket_receiver,
            std::time::Duration::from_millis(50),
        ));

        assert!(
            receive_public_invocation_start(
                &mut websocket_stream,
                &websocket_sender,
                &mut InvocationSessionState::default(),
            )
            .await
            .is_none()
        );
        drop(websocket_sender);
        tokio::time::timeout(std::time::Duration::from_millis(500), writer)
            .await
            .expect("pre-start writer exceeded its progress deadline")
            .unwrap();
        assert!(delivered.lock().await.is_empty());
        drop(permit_sender);
    }

    #[test]
    async fn response_completion_waits_for_temporarily_backpressured_writer() {
        let delivered = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let (permit_sender, permit_receiver) = tokio::sync::mpsc::channel(3);
        let sink = Box::pin(gated_websocket_sink(permit_receiver, delivered.clone()));
        let (websocket_sender, websocket_receiver) = tokio::sync::mpsc::channel(3);
        let writer = tokio::spawn(forward_websocket_messages(
            sink,
            websocket_receiver,
            std::time::Duration::from_secs(5),
        ));
        let requests = tokio::spawn(futures::future::pending());
        let responses = tokio::spawn(async move {
            let accepted = InvocationResponse {
                response: Some(invocation_response::Response::Accepted(Default::default())),
            };
            let finished = InvocationResponse {
                response: Some(invocation_response::Response::Finished(
                    InvocationSessionCompletion::default(),
                )),
            };
            websocket_sender
                .send(Message::binary(accepted.encode_to_vec()))
                .await
                .unwrap();
            websocket_sender
                .send(Message::binary(finished.encode_to_vec()))
                .await
                .unwrap();
            websocket_sender
                .send(Message::close_with(CloseCode::Normal, "session complete"))
                .await
                .unwrap();
        });

        let supervisor = tokio::spawn(supervise_public_invocation_session(
            requests, responses, writer,
        ));
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        assert!(
            !supervisor.is_finished(),
            "temporary output backpressure truncated queued semantic responses"
        );

        for _ in 0..3 {
            permit_sender.send(()).await.unwrap();
        }
        tokio::time::timeout(std::time::Duration::from_secs(2), supervisor)
            .await
            .expect("writer did not drain after output progress resumed")
            .unwrap();

        let delivered = delivered.lock().await;
        assert_eq!(delivered.len(), 3);
        let Message::Binary(accepted) = &delivered[0] else {
            panic!("accepted response was not delivered first")
        };
        assert!(matches!(
            InvocationResponse::decode(accepted.as_slice())
                .unwrap()
                .response,
            Some(invocation_response::Response::Accepted(_))
        ));
        let Message::Binary(finished) = &delivered[1] else {
            panic!("finished response was not delivered second")
        };
        assert!(matches!(
            InvocationResponse::decode(finished.as_slice())
                .unwrap()
                .response,
            Some(invocation_response::Response::Finished(_))
        ));
        assert!(matches!(delivered[2], Message::Close(_)));
    }

    #[test]
    async fn permanently_stalled_writer_terminates_at_progress_deadline() {
        let delivered = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let (permit_sender, permit_receiver) = tokio::sync::mpsc::channel(1);
        let sink = Box::pin(gated_websocket_sink(permit_receiver, delivered.clone()));
        let (websocket_sender, websocket_receiver) = tokio::sync::mpsc::channel(2);
        let writer = tokio::spawn(forward_websocket_messages(
            sink,
            websocket_receiver,
            std::time::Duration::from_millis(50),
        ));
        let requests = tokio::spawn(futures::future::pending());
        let responses = tokio::spawn(async move {
            websocket_sender
                .send(Message::binary(
                    InvocationResponse {
                        response: Some(invocation_response::Response::Finished(
                            InvocationSessionCompletion::default(),
                        )),
                    }
                    .encode_to_vec(),
                ))
                .await
                .unwrap();
            websocket_sender
                .send(Message::close_with(CloseCode::Normal, "session complete"))
                .await
                .unwrap();
        });

        tokio::time::timeout(
            std::time::Duration::from_millis(500),
            supervise_public_invocation_session(requests, responses, writer),
        )
        .await
        .expect("permanently stalled writer exceeded its progress deadline");
        assert!(delivered.lock().await.is_empty());
        drop(permit_sender);
    }

    #[test]
    async fn invocation_responses_are_forwarded_as_binary_protobuf_messages() {
        let response = InvocationResponse { response: None };
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);

        assert!(queue_invocation_response(&sender, response.clone()).await);
        let message = receiver.recv().await.unwrap();
        let Message::Binary(bytes) = message else {
            panic!("invocation response must use a binary WebSocket message")
        };

        assert_eq!(
            InvocationResponse::decode(bytes.as_slice()).unwrap(),
            response
        );
    }

    #[test]
    fn websocket_close_reasons_are_utf8_safe_and_protocol_sized() {
        let reason = "é".repeat(100);
        let bounded = bounded_close_reason(&reason);

        assert!(bounded.len() <= 123);
        assert!(reason.starts_with(&bounded));
    }

    #[test]
    fn public_resolution_errors_map_to_protocol_rejections() {
        assert_eq!(
            rejection_reason(&WorkerServiceError::TypeChecker("invalid".to_string())),
            InvocationRejectionReason::Validation
        );
        assert_eq!(
            rejection_reason(&WorkerServiceError::RegistryServiceError(
                RegistryServiceError::Unauthorized("forbidden".to_string())
            )),
            InvocationRejectionReason::Unauthorized
        );
        assert_eq!(
            rejection_reason(&WorkerServiceError::RegistryServiceError(
                RegistryServiceError::NotFound("missing".to_string())
            )),
            InvocationRejectionReason::NotFound
        );
    }
}
