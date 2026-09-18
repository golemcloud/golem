// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use super::error::RequestHandlerError;
use super::file_response::{FileRequest, representation_headers, verified_stream};
use super::route_resolver::ResolvedRouteEntry;
use super::{ResponseBody, RichRequest, RouteExecutionResult};
use crate::service::worker::{WorkerService, WorkerServiceError};
use futures::StreamExt;
use golem_common::model::AgentId;
use golem_common::model::filesystem::{FileByteSelection, FileReadError, FileReadHead};
use http::{HeaderMap, HeaderValue, Method, StatusCode, header};

pub(super) async fn serve(
    worker: &WorkerService,
    request: &RichRequest,
    selected: &ResolvedRouteEntry,
    agent_id: &AgentId,
    path: &str,
    directory_request: bool,
) -> Result<Option<RouteExecutionResult>, RequestHandlerError> {
    let plan = FileRequest::new(
        request.underlying.method(),
        request.underlying.headers(),
        None,
    );
    let selection = if directory_request {
        FileByteSelection::MetadataOnly
    } else {
        plan.as_ref()
            .map(|plan| plan.selection)
            .unwrap_or(FileByteSelection::MetadataOnly)
    };
    let read = match worker
        .read_mounted_file(
            agent_id,
            path,
            selection,
            selected.route.environment_id,
            selected.route.account_id,
        )
        .await
    {
        Ok(read) => read,
        Err(WorkerServiceError::FileRead(FileReadError::ResourceExhausted)) => {
            return Ok(Some(empty_response(StatusCode::SERVICE_UNAVAILABLE)));
        }
        Err(error) => return Err(RequestHandlerError::InternalError(error.into())),
    };
    let metadata = match read.head {
        FileReadHead::Absent => return Ok(None),
        FileReadHead::NotRegular | FileReadHead::Symlink | FileReadHead::PermissionDenied => {
            return Ok(Some(empty_response(StatusCode::FORBIDDEN)));
        }
        FileReadHead::File(_) if directory_request => {
            return Ok(Some(empty_response(StatusCode::FORBIDDEN)));
        }
        FileReadHead::File(metadata) => metadata,
    };
    // Preconditions cannot hide absence or forbidden targets or skip another mapping.
    let plan = plan?;
    let (status, extent) = plan.response(metadata.total_size, metadata.selection);
    let mut headers = representation_headers(path, metadata.total_size, status, extent);
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    let body =
        if *request.underlying.method() == Method::HEAD || !status.is_success() || extent.1 == 0 {
            ResponseBody::NoBody
        } else {
            ResponseBody::Stream(poem::Body::from_bytes_stream(verified_stream(
                read.body
                    .map(|item| item.map_err(anyhow::Error::from))
                    .boxed(),
                extent.1,
            )))
        };
    Ok(Some(RouteExecutionResult {
        status,
        headers,
        body,
    }))
}

fn empty_response(status: StatusCode) -> RouteExecutionResult {
    RouteExecutionResult {
        status,
        headers: HeaderMap::from_iter([(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        )]),
        body: ResponseBody::NoBody,
    }
}
