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

use super::{
    Future, HashMap, InvocationResponse, SessionError, StreamSession, mpsc, send_protocol_failure,
};

pub(super) async fn detach_durable_attachment(streams: Option<StreamSession>) {
    if let Some(streams) = streams
        && let Err(error) = streams.detach_current().await
    {
        tracing::warn!(%error, "failed to persist durable invocation transport detach");
    }
}

pub(super) async fn until_response_closed<T>(
    outward: &mpsc::Sender<InvocationResponse>,
    forwarder_stopped: &tokio_util::sync::CancellationToken,
    operation: impl std::future::Future<Output = T>,
) -> Option<T> {
    tokio::select! {
        result = operation => Some(result),
        _ = outward.closed() => None,
        _ = forwarder_stopped.cancelled() => None,
    }
}

pub(super) fn is_attachment_termination(error: &SessionError) -> bool {
    error.contains("response stream closed") || error.starts_with("StaleEpoch:")
}

/// Whether `error` ends only this attachment and leaves the session to a later one: the attachment
/// terminated, or the storage did not answer a read of the session history. The history stays
/// intact, so the client resumes from its durable cursors.
pub(super) fn ends_only_the_attachment(error: &SessionError) -> bool {
    match error {
        SessionError::Unreadable(unreadable) => unreadable.is_transient(),
        SessionError::Fenced(_) => false,
        SessionError::Failed(_) => is_attachment_termination(error),
    }
}

/// Ends the attachment on a failed durable session operation. Only a violation of the session
/// protocol records a failed session terminal. A refused write and an oplog that does not read
/// are failures of the shard or of the agent, not of the session: they go out as the worker error
/// they are and record nothing, so the session result stays with the history.
pub(super) async fn end_attachment_on_failure(
    streams: &StreamSession,
    responses: &mpsc::Sender<InvocationResponse>,
    high_waters: &HashMap<u64, golem_common::base_model::durable_stream::InputStreamHighWater>,
    error: SessionError,
    drain_outputs: impl Future<Output = ()>,
) {
    if ends_only_the_attachment(&error) {
        if matches!(error, SessionError::Unreadable(_)) {
            tracing::warn!(
                error = %error,
                "Ending the invocation session attachment because the session history did not read"
            );
        }
        return;
    }
    if !matches!(error, SessionError::Failed(_)) {
        send_protocol_failure(responses, error).await;
        return;
    }
    if let Err(finish_error) = streams.fail_protocol(error.to_string()).await {
        send_protocol_failure(responses, finish_error).await;
        return;
    }
    let _ = streams
        .pump_input_cancellations(responses, high_waters)
        .await;
    drain_outputs.await;
    send_protocol_failure(responses, error).await;
}
