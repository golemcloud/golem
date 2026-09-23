// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use super::super::error::RequestHandlerError;
use super::super::route_resolver::ResolvedRouteEntry;
use super::super::{RichRequest, RouteExecutionResult};
use super::encoding::metadata_response;
use super::{DurableStreamsHandler, response, route_method};
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    ForkStreamSlotRequest, StreamSessionExpiryPolicy, fork_stream_slot_rejection,
    fork_stream_slot_response,
};
use golem_common::model::AgentId;
use golem_common::model::durable_stream::StreamOffset;
use golem_common::model::invocation_session_public::validate_durable_stream_session_id;
use golem_service_base::custom_api::CallAgentBehaviour;
use golem_service_base::model::auth::AuthCtx;
use http::StatusCode;
use std::str::FromStr;
use tokio::io::AsyncReadExt;
use uuid::Uuid;

impl DurableStreamsHandler {
    pub(super) async fn fork(
        &self,
        request: &mut RichRequest,
        route: &ResolvedRouteEntry,
        behaviour: &CallAgentBehaviour,
        root_agent_id: &AgentId,
        target_agent_id: &AgentId,
        target_fork: &str,
        session: &str,
        slot: &str,
        expiry_policy: Option<StreamSessionExpiryPolicy>,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        if self.forks.max_forks_per_second == 0 {
            return Ok(response(StatusCode::CONFLICT));
        }
        macro_rules! header {
            ($name:literal) => {
                match single_header(request, $name) {
                    Ok(value) => value,
                    Err(_) => return Ok(response(StatusCode::BAD_REQUEST)),
                }
            };
        }
        let Some(source) = header!("stream-forked-from") else {
            return Ok(response(StatusCode::BAD_REQUEST));
        };
        let parsed = match parse_source_path(
            request.underlying.uri().path(),
            &source,
            target_fork,
            session,
            slot,
        ) {
            Ok(parsed) => parsed,
            Err(_) => return Ok(response(StatusCode::BAD_REQUEST)),
        };
        let source_agent_id = match parsed.source_fork.as_deref() {
            Some(fork) => super::CallAgentHandler::build_agent_id(
                route,
                behaviour.component_id,
                &behaviour.agent_type,
                &behaviour.constructor_input,
                &behaviour.constructor_parameters,
                Some(fork_phantom_id(root_agent_id, fork)),
            )?,
            None => root_agent_id.clone(),
        };
        let fork_offset = match header!("stream-fork-offset") {
            None => None,
            Some(value) => match StreamOffset::from_str(&value) {
                Ok(offset) => Some(offset.as_bytes().to_vec()),
                Err(_) => return Ok(response(StatusCode::BAD_REQUEST)),
            },
        };
        let sub_offset = match header!("stream-fork-sub-offset")
            .map(|value| value.parse::<u64>())
            .transpose()
        {
            Ok(value) => value.unwrap_or(0),
            Err(_) => return Ok(response(StatusCode::BAD_REQUEST)),
        };
        let content_type = header!("content-type").map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_owned()
        });
        let closed =
            header!("stream-closed").is_some_and(|value| value.eq_ignore_ascii_case("true"));
        let mut body = request
            .underlying
            .take_body()
            .into_async_read()
            .take((self.max_append_body_bytes + 1) as u64);
        let mut initial_content = Vec::new();
        body.read_to_end(&mut initial_content)
            .await
            .map_err(anyhow::Error::from)?;
        if initial_content.len() > self.max_append_body_bytes {
            return Ok(response(StatusCode::PAYLOAD_TOO_LARGE));
        }
        let result = self
            .worker_service
            .fork_stream_slot(
                &source_agent_id,
                ForkStreamSlotRequest {
                    source_agent_id: Some(source_agent_id.clone().into()),
                    target_agent_id: Some(target_agent_id.clone().into()),
                    environment_id: Some(route.route.environment_id.into()),
                    auth_ctx: Some(AuthCtx::System.into()),
                    session: session.to_owned(),
                    slot: slot.to_owned(),
                    expected_method: route_method(route).to_owned(),
                    source_path: parsed.canonical,
                    fork_offset,
                    sub_offset,
                    content_type,
                    max_forks_per_session: self.forks.max_forks_per_session,
                    max_forks_per_second: self.forks.max_forks_per_second,
                    max_copied_bytes: self.forks.max_copied_bytes,
                    initial_content,
                    closed,
                    expiry_policy,
                },
            )
            .await?;
        match result {
            fork_stream_slot_response::Result::Success(success) => {
                let metadata = self
                    .read_slot_admitted(
                        route,
                        target_agent_id,
                        session,
                        slot,
                        Vec::new(),
                        0,
                        0,
                        golem_api_grpc::proto::golem::workerexecutor::v1::StreamSlotReadAdmission::Continuation,
                        success.invocation_key.clone(),
                    )
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("fork succeeded without stream metadata"))?;
                if metadata.tombstoned {
                    return Ok(response(StatusCode::CONFLICT));
                }
                let mut out = metadata_response(&metadata, true)?;
                out.status = if success.replayed {
                    StatusCode::OK
                } else {
                    StatusCode::CREATED
                };
                out.headers.insert(
                    http::header::LOCATION,
                    request
                        .underlying
                        .uri()
                        .path()
                        .parse()
                        .map_err(anyhow::Error::from)?,
                );
                Ok(out)
            }
            fork_stream_slot_response::Result::Rejected(rejected) => {
                let status = match fork_stream_slot_rejection::Reason::try_from(rejected.reason) {
                    Ok(fork_stream_slot_rejection::Reason::NotFound) => StatusCode::NOT_FOUND,
                    Ok(fork_stream_slot_rejection::Reason::Conflict) => StatusCode::CONFLICT,
                    Ok(fork_stream_slot_rejection::Reason::InvalidOffset) => {
                        StatusCode::BAD_REQUEST
                    }
                    Ok(fork_stream_slot_rejection::Reason::TooLarge) => {
                        StatusCode::PAYLOAD_TOO_LARGE
                    }
                    Ok(fork_stream_slot_rejection::Reason::RateLimited) => {
                        StatusCode::TOO_MANY_REQUESTS
                    }
                    Ok(fork_stream_slot_rejection::Reason::ReadOnly) => StatusCode::FORBIDDEN,
                    _ => return Err(anyhow::anyhow!("unspecified fork rejection").into()),
                };
                let mut out = response(status);
                if status == StatusCode::TOO_MANY_REQUESTS && rejected.retry_after_seconds > 0 {
                    out.headers.insert(
                        http::header::RETRY_AFTER,
                        rejected.retry_after_seconds.into(),
                    );
                }
                Ok(out)
            }
            fork_stream_slot_response::Result::Failure(error) => {
                let error =
                    golem_service_base::error::worker_executor::WorkerExecutorError::try_from(
                        error,
                    )
                    .map_err(anyhow::Error::msg)?;
                Err(crate::service::worker::WorkerServiceError::from(error).into())
            }
        }
    }
}

fn single_header(request: &RichRequest, name: &str) -> Result<Option<String>, RequestHandlerError> {
    let values = request.headers().get_all(name);
    if values.iter().count() > 1 {
        return Err(anyhow::anyhow!("multiple {name} headers").into());
    }
    values
        .iter()
        .next()
        .map(|value| {
            value
                .to_str()
                .map(str::to_owned)
                .map_err(|_| RequestHandlerError::HeaderIsNotAscii {
                    header_name: name.to_owned(),
                })
        })
        .transpose()
}

struct ForkSource {
    source_fork: Option<String>,
    canonical: String,
}

fn parse_source_path(
    current: &str,
    source: &str,
    target_fork: &str,
    session: &str,
    slot: &str,
) -> Result<ForkSource, RequestHandlerError> {
    if !source.starts_with('/') || source.contains(['?', '#']) || source.contains("//") {
        return Err(anyhow::anyhow!("invalid fork source path").into());
    }
    let current = urlencoding::decode(current)
        .map_err(|_| anyhow::anyhow!("invalid percent encoding in fork target path"))?;
    let source = urlencoding::decode(source)
        .map_err(|_| anyhow::anyhow!("invalid percent encoding in fork source path"))?;
    if source.contains(['?', '#'])
        || source.contains("//")
        || source.split('/').any(|s| s == "." || s == "..")
    {
        return Err(anyhow::anyhow!("invalid fork source path").into());
    }
    let marker = format!("/forks/{target_fork}/invocations/{session}/streams/{slot}");
    let base = current
        .strip_suffix(&marker)
        .ok_or_else(|| anyhow::anyhow!("invalid fork target path"))?;
    let normal = format!("{base}/invocations/{session}/streams/{slot}");
    if source == normal {
        return Ok(ForkSource {
            source_fork: None,
            canonical: canonical_path(&source),
        });
    }
    let prefix = format!("{base}/forks/");
    let suffix = format!("/invocations/{session}/streams/{slot}");
    let fork = source
        .strip_prefix(&prefix)
        .and_then(|s| s.strip_suffix(&suffix))
        .filter(|s| !s.contains('/') && validate_durable_stream_session_id(s).is_ok())
        .ok_or_else(|| {
            anyhow::anyhow!("fork source must identify the same route, session, and slot")
        })?;
    Ok(ForkSource {
        source_fork: Some(fork.to_owned()),
        canonical: canonical_path(&source),
    })
}

fn canonical_path(path: &str) -> String {
    path.split('/')
        .map(|segment| urlencoding::encode(segment).into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

pub(super) fn fork_phantom_id(root: &AgentId, fork: &str) -> Uuid {
    let hash = blake3::hash(format!("{}\0{fork}", root.agent_id).as_bytes());
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&hash.as_bytes()[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}
