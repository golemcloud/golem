// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use super::error::RequestHandlerError;
use super::file_response::{representation_headers, select, verified_stream};
use super::{ResponseBody, RouteExecutionResult};
use golem_common::model::environment::EnvironmentId;
use golem_service_base::custom_api::RouterFileIndexEntry;
use golem_service_base::service::initial_agent_files::InitialAgentFilesService;
use http::{HeaderMap, HeaderValue, Method, header};

pub(super) async fn serve(
    files: &InitialAgentFilesService,
    environment_id: EnvironmentId,
    entry: &RouterFileIndexEntry,
    method: &Method,
    request_headers: &HeaderMap,
) -> Result<RouteExecutionResult, RequestHandlerError> {
    let etag = format!("\"blake3-{}\"", entry.blob_key.0.into_blake3().to_hex());
    let (status, selection) = select(method, request_headers, etag.as_bytes(), entry.size)?;
    let mut headers = representation_headers(&entry.path, entry.size, status, selection);
    headers.insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    let body = if *method == Method::HEAD || !status.is_success() || selection.1 == 0 {
        // Even conditional responses must not hide a broken deployment index.
        let metadata = files
            .get_metadata(environment_id, entry.blob_key)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Indexed initial file blob is missing"))?;
        if metadata.size != entry.size {
            return Err(anyhow::anyhow!(
                "Initial file size differs from deployment index: expected {}, got {}",
                entry.size,
                metadata.size
            )
            .into());
        }
        ResponseBody::NoBody
    } else {
        let opened = files
            .get_range(environment_id, entry.blob_key, selection.0, selection.1)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Indexed initial file blob is missing"))?;
        if opened.total_size != entry.size {
            return Err(anyhow::anyhow!(
                "Initial file size differs from deployment index: expected {}, got {}",
                entry.size,
                opened.total_size
            )
            .into());
        }
        ResponseBody::Stream(poem::Body::from_bytes_stream(verified_stream(
            opened.stream,
            selection.1,
        )))
    };
    Ok(RouteExecutionResult {
        status,
        headers,
        body,
    })
}

#[cfg(test)]
mod tests;
