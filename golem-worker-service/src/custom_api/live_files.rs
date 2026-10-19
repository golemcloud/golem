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
use crate::service::worker::WorkerService;
use bytes::Bytes;
use futures::{StreamExt, stream::BoxStream};
use golem_common::model::AgentId;
use golem_common::model::filesystem::{FileByteSelection, FileReadHead};
use http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use std::io;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use tokio_util::task::AbortOnDropHandle;

pub(super) async fn serve(
    worker: &WorkerService,
    request: &RichRequest,
    selected: &ResolvedRouteEntry,
    agent_id: &AgentId,
    path: &str,
    directory_request: bool,
    deadline: Instant,
) -> Result<Option<RouteExecutionResult>, RequestHandlerError> {
    if Instant::now() >= deadline {
        return Err(RequestHandlerError::RawDeadline);
    }
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
        Err(error) => return Err(RequestHandlerError::InternalError(error.into())),
    };
    if Instant::now() >= deadline {
        return Err(RequestHandlerError::RawDeadline);
    }
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
            ResponseBody::Stream(poem::Body::from_bytes_stream(owned_stream(
                verified_stream(
                    read.body
                        .map(|item| item.map_err(anyhow::Error::from))
                        .boxed(),
                    extent.1,
                ),
                deadline,
            )))
        };
    Ok(Some(RouteExecutionResult {
        status,
        headers,
        body,
    }))
}

/// The owner watches cancellation even when the HTTP transport stops polling.
/// One queued chunk plus one in flight bounds read-ahead; the gRPC decoder bounds chunks.
fn owned_stream(
    mut source: BoxStream<'static, Result<Bytes, io::Error>>,
    deadline: Instant,
) -> BoxStream<'static, Result<Bytes, io::Error>> {
    let (sender, receiver) = mpsc::channel(1);
    // Terminal errors must not need a free slot in the backpressured data queue.
    let (completed, completion) = oneshot::channel();
    let owner = AbortOnDropHandle::new(tokio::spawn(async move {
        let outcome = tokio::select! {
            biased;
            _ = tokio::time::sleep_until(deadline) => {
                Err(io::Error::new(io::ErrorKind::TimedOut, "File response timed out"))
            }
            _ = sender.closed() => return,
            result = async {
                while let Some(bytes) = source.next().await {
                    sender.send(bytes?).await.map_err(|_| {
                        io::Error::new(io::ErrorKind::BrokenPipe, "File response closed")
                    })?;
                }
                Ok(())
            } => result,
        };
        drop(source);
        let _ = completed.send(outcome);
    }));
    futures::stream::unfold(Some((receiver, completion, owner)), |state| async move {
        let (mut receiver, completion, owner) = state?;
        if let Some(bytes) = receiver.recv().await {
            return Some((Ok(bytes), Some((receiver, completion, owner))));
        }
        match completion.await {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some((Err(error), None)),
            Err(_) => Some((Err(io::Error::other("File response owner failed")), None)),
        }
    })
    .boxed()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use test_r::test;

    struct Dropped(Option<oneshot::Sender<()>>);

    impl Drop for Dropped {
        fn drop(&mut self) {
            let _ = self.0.take().unwrap().send(());
        }
    }

    #[test]
    fn deadline_and_drop_release_unpolled_and_backpressured_sources() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                tokio::time::pause();
                for backpressure in [false, true] {
                    for disconnect in [false, true] {
                        let (dropped, finished) = oneshot::channel();
                        let guard = Dropped(Some(dropped));
                        let polls = Arc::new(AtomicUsize::new(0));
                        let count = polls.clone();
                        let source = futures::stream::poll_fn(move |_| {
                            let _ = &guard;
                            count.fetch_add(1, Ordering::SeqCst);
                            if backpressure {
                                std::task::Poll::Ready(Some(Ok(Bytes::from_static(b"abc"))))
                            } else {
                                std::task::Poll::Pending
                            }
                        })
                        .boxed();
                        let body = owned_stream(source, Instant::now() + Duration::from_secs(7));
                        tokio::task::yield_now().await;
                        assert_eq!(
                            polls.load(Ordering::SeqCst),
                            if backpressure { 2 } else { 1 }
                        );
                        if disconnect {
                            drop(body);
                            finished.await.unwrap();
                        } else {
                            tokio::time::advance(Duration::from_secs(7)).await;
                            // Cleanup must happen before the HTTP body is polled again.
                            finished.await.unwrap();
                            let output = body.collect::<Vec<_>>().await;
                            assert_eq!(output.len(), if backpressure { 2 } else { 1 });
                            assert_eq!(
                                output.last().unwrap().as_ref().unwrap_err().kind(),
                                io::ErrorKind::TimedOut
                            );
                        }
                    }
                }
            });
    }

    #[test]
    async fn owned_stream_preserves_verified_completion_and_safe_failure() {
        for (bytes, length, late_error) in [
            (b"abc".as_slice(), 3, false),
            (b"abc".as_slice(), 4, false),
            (b"abcd".as_slice(), 3, false),
            (b"abc".as_slice(), 3, true),
        ] {
            let mut chunks = vec![Ok(Bytes::copy_from_slice(bytes))];
            if late_error {
                chunks.push(Err(anyhow::anyhow!("/private storage failure")));
            }
            let output = owned_stream(
                verified_stream(futures::stream::iter(chunks).boxed(), length),
                Instant::now() + Duration::from_secs(5),
            )
            .collect::<Vec<_>>()
            .await;
            let received = output
                .iter()
                .filter_map(|item| item.as_ref().ok())
                .cloned()
                .collect::<Vec<_>>()
                .concat();
            if bytes.len() as u64 == length && !late_error {
                assert!(output.iter().all(Result::is_ok));
                assert_eq!(received, b"abc");
            } else {
                assert!(received.len() < length as usize);
                let error = output.last().unwrap().as_ref().unwrap_err();
                assert!(!error.to_string().contains("private"));
            }
        }
    }
}
