// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! `GET`/`HEAD` of a single stream slot: catch-up pages, long-poll and SSE
//! live reads, and conditional revalidation of closed pages.

use super::super::error::RequestHandlerError;
use super::super::route_resolver::ResolvedRouteEntry;
use super::super::{ResponseBody, RichRequest, RouteExecutionResult};
use super::encoding::{
    content_type, data_response, etag, live_cursor, metadata_response, offset_text, sse_batch,
};
use super::load::LiveReaderPermit;
use super::{
    DurableStreamsHandler, MAX_BYTES, MAX_ITEMS, rejection_response, response, route_method,
};
use crate::service::worker::WorkerService;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    ReadStreamSlotRequest, ReadStreamSlotSuccess,
};
use golem_common::model::durable_stream::StreamOffset;
use golem_common::model::{AgentId, OplogIndex};
use golem_service_base::model::auth::AuthCtx;
use http::{HeaderName, HeaderValue, Method, StatusCode};
use std::str::FromStr;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Live {
    LongPoll,
    Sse,
}

impl DurableStreamsHandler {
    pub(super) async fn read(
        &self,
        request: &RichRequest,
        route: &ResolvedRouteEntry,
        agent_id: &AgentId,
        session: String,
        slot: String,
    ) -> Result<RouteExecutionResult, RequestHandlerError> {
        if request.underlying.method() == Method::HEAD {
            let read = self
                .read_slot(route, agent_id, &session, &slot, Vec::new(), 0, 0)
                .await?;
            return Ok(read
                .map(|r| metadata_response(&r, true))
                .transpose()?
                .unwrap_or_else(|| response(StatusCode::NOT_FOUND)));
        }
        if ["offset", "live", "cursor"].iter().any(|key| {
            request
                .query_params()
                .get(*key)
                .is_some_and(|values| values.len() != 1)
        }) {
            return Ok(response(StatusCode::BAD_REQUEST));
        }
        let live = match single_query_param(request, "live") {
            None => None,
            Some("long-poll") => Some(Live::LongPoll),
            Some("sse") => Some(Live::Sse),
            Some(_) => return Ok(response(StatusCode::BAD_REQUEST)),
        };
        let cursor = match single_query_param(request, "cursor") {
            Some(value) => match value.parse::<u64>() {
                Ok(value) if value <= u64::MAX - 180 => Some(value),
                _ => return Ok(response(StatusCode::BAD_REQUEST)),
            },
            None => None,
        };
        let offset = single_query_param(request, "offset").unwrap_or("-1");
        let mut tail = None;
        let from = match offset {
            "-1" => Vec::new(),
            "now" => {
                let Some(mut head) = self
                    .read_slot(route, agent_id, &session, &slot, Vec::new(), 0, 0)
                    .await?
                else {
                    return Ok(response(StatusCode::NOT_FOUND));
                };
                if head.tombstoned {
                    return Ok(response(StatusCode::GONE));
                }
                head.next_offset = head.head_offset.clone();
                head.up_to_date = true;
                let from = head.head_offset.clone();
                tail = Some(head);
                from
            }
            value => match StreamOffset::from_str(value) {
                Ok(v) if v == StreamOffset::new(OplogIndex::NONE, 0) => Vec::new(),
                Ok(v) => v.as_bytes().to_vec(),
                Err(_) => return Ok(response(StatusCode::BAD_REQUEST)),
            },
        };
        let key = format!("{}:{agent_id}:{session}:{slot}", route.route.environment_id);
        let permit = if live.is_some() {
            match self.limiter.try_acquire_reader(&key) {
                Ok(permit) => Some(permit),
                Err(rejection) => return Ok(rejection_response(rejection)),
            }
        } else {
            if let Err(rejection) = self.limiter.check_catch_up(&key) {
                return Ok(rejection_response(rejection));
            }
            None
        };
        let wait = live
            .map(|_| self.long_poll_timeout.as_millis() as u64)
            .unwrap_or(0);
        let start_offset = offset_text(&from)?;
        let read = match tail {
            Some(head) if live != Some(Live::LongPoll) || head.closed => head,
            _ => match self
                .read_slot(route, agent_id, &session, &slot, from, MAX_ITEMS, wait)
                .await?
            {
                Some(read) => read,
                None => return Ok(response(StatusCode::NOT_FOUND)),
            },
        };
        if read.tombstoned {
            return Ok(response(StatusCode::GONE));
        }
        if live == Some(Live::LongPoll) && read.items.is_empty() {
            let mut r = metadata_response(&read, false)?;
            r.status = StatusCode::NO_CONTENT;
            r.headers.insert(
                HeaderName::from_static("stream-cursor"),
                HeaderValue::from(live_cursor(cursor)?),
            );
            return Ok(r);
        }
        if live == Some(Live::Sse) {
            let mut result = metadata_response(&read, false)?;
            result.headers.insert(
                http::header::CONTENT_TYPE,
                HeaderValue::from_static("text/event-stream"),
            );
            if content_type(&read) == "application/octet-stream" {
                result.headers.insert(
                    HeaderName::from_static("stream-sse-data-encoding"),
                    HeaderValue::from_static("base64"),
                );
            }
            let read_request = ReadStreamSlotRequest {
                agent_id: Some(agent_id.clone().into()),
                environment_id: Some(route.route.environment_id.into()),
                auth_ctx: Some(AuthCtx::System.into()),
                session,
                slot,
                from_offset: Vec::new(),
                max_items: MAX_ITEMS,
                max_bytes: MAX_BYTES,
                wait_millis: wait,
                expected_method: route_method(route).to_owned(),
            };
            result.body = ResponseBody::PoemBody {
                body: sse_body(
                    self.worker_service.clone(),
                    agent_id.clone(),
                    read_request,
                    read,
                    permit,
                    cursor,
                ),
                content_type: Some("text/event-stream"),
            };
            return Ok(result);
        }
        let mut result = data_response(&read)?;
        let etag = etag(&read, &start_offset, &read.next_offset)?;
        result.headers.insert(
            http::header::ETAG,
            etag.parse().map_err(anyhow::Error::from)?,
        );
        if live.is_some() {
            result.headers.insert(
                http::header::CACHE_CONTROL,
                HeaderValue::from_static("no-store"),
            );
            result.headers.insert(
                HeaderName::from_static("stream-cursor"),
                HeaderValue::from(live_cursor(cursor)?),
            );
        } else if read.closed && if_none_match_hits(request, &etag) {
            result.status = StatusCode::NOT_MODIFIED;
            result.body = ResponseBody::NoBody;
        }
        Ok(result)
    }
}

fn single_query_param<'a>(request: &'a RichRequest, key: &str) -> Option<&'a str> {
    request
        .query_params()
        .get(key)
        .and_then(|v| v.first())
        .map(String::as_str)
}

fn if_none_match_hits(request: &RichRequest, etag: &str) -> bool {
    request
        .headers()
        .get_all(http::header::IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|v| v.trim() == "*" || v.trim().trim_start_matches("W/") == etag)
}

/// Streams SSE frames starting with the already fetched `first` batch, then
/// keeps reading from the executor until the stream is closed and up to date
/// or disappears. The live reader permit is held for the life of the body.
fn sse_body(
    service: Arc<WorkerService>,
    agent_id: AgentId,
    read_request: ReadStreamSlotRequest,
    first: ReadStreamSlotSuccess,
    permit: Option<LiveReaderPermit>,
    cursor: Option<u64>,
) -> poem::Body {
    let stream = futures::stream::try_unfold(
        (Some(first), read_request, permit, false, cursor),
        move |(next, mut request, permit, done, mut cursor)| {
            let service = service.clone();
            let agent_id = agent_id.clone();
            async move {
                if done {
                    return Ok(None);
                }
                let batch = match next {
                    Some(batch) => batch,
                    None => match service
                        .read_stream_slot(&agent_id, request.clone())
                        .await
                        .map_err(std::io::Error::other)?
                    {
                        Some(batch) => batch,
                        None => return Ok(None),
                    },
                };
                if batch.tombstoned {
                    return Ok(None);
                }
                let closed = batch.closed && batch.up_to_date;
                request.from_offset = batch.next_offset.clone();
                let data = sse_batch(&batch, &mut cursor).map_err(std::io::Error::other)?;
                Ok::<_, std::io::Error>(Some((
                    bytes::Bytes::from(data),
                    (None, request, permit, closed, cursor),
                )))
            }
        },
    );
    poem::Body::from_bytes_stream(stream)
}
