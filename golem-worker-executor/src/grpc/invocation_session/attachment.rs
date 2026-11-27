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

use crate::worker::InvocationObservationFailure;
use futures::StreamExt;

use super::{
    Future, HashMap, InvocationResponse, SessionError, StreamSession, WorkerExecutorError, mpsc,
    send_protocol_failure,
};

async fn detach_durable_attachment(streams: Option<StreamSession>) {
    if let Some(streams) = streams
        && let Err(error) = streams.detach_current().await
    {
        tracing::warn!(%error, "failed to persist durable invocation transport detach");
    }
}

/// Who may write the transport detach of a durable attachment when Start or Resume ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CleanupPhase {
    /// This transport holds the attachment and writes its detach when it ends.
    Held,
    /// A read of the session history failed for a time. The history did not show what this
    /// attachment may write, so the attachment stays as it is: no detach and no terminal.
    Unsettled,
    /// Another owner settles the attachment: a revocation, a takeover, a failed authority check,
    /// a failed observation, or an observer that never held the attachment.
    Released,
}

/// An event that can change the [`CleanupPhase`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CleanupEvent {
    /// A read of the stored session history or of the agent oplog failed for a time.
    ReadFailed,
    /// The attachment ended because of a revocation, an authority check or a failed observation.
    AuthorityEnded,
    /// An operation failed for a reason that does not change the cleanup authority.
    OtherFailure,
}

impl CleanupEvent {
    /// The event of a failed observation of the invocation. Only a read that failed for a time
    /// or a reroute changes the cleanup authority.
    pub(super) fn of_observation_failure(error: &WorkerExecutorError) -> Self {
        match InvocationObservationFailure::classify(error) {
            Some(InvocationObservationFailure::RetryableRead) => Self::ReadFailed,
            Some(InvocationObservationFailure::Reroute) => Self::AuthorityEnded,
            _ => Self::OtherFailure,
        }
    }

    /// The event of a resume that fails after its queued attachment became ready. A read that
    /// failed for a time leaves the attachment unsettled. Any other failure leaves the attachment
    /// to the worker that refused the resume. So the transport writes no detach.
    pub(super) fn of_failed_resume(error: &WorkerExecutorError) -> Self {
        match Self::of_observation_failure(error) {
            Self::ReadFailed => Self::ReadFailed,
            Self::AuthorityEnded | Self::OtherFailure => Self::AuthorityEnded,
        }
    }

    /// The event of a failed session operation. Only a read that failed for a time is a
    /// [`CleanupEvent::ReadFailed`].
    pub(super) fn of_session_error(error: &SessionError) -> Self {
        match error {
            SessionError::Unreadable(unreadable) if unreadable.is_transient() => Self::ReadFailed,
            SessionError::Unreadable(_)
            | SessionError::Fenced(_)
            | SessionError::IncompleteOutput(_)
            | SessionError::Failed(_) => Self::OtherFailure,
        }
    }
}

/// The phase after `event`. Only a held attachment changes phase; the first event that ends
/// the hold decides the phase.
pub(super) fn cleanup_transition(phase: CleanupPhase, event: CleanupEvent) -> CleanupPhase {
    match (phase, event) {
        (CleanupPhase::Held, CleanupEvent::ReadFailed) => CleanupPhase::Unsettled,
        (CleanupPhase::Held, CleanupEvent::AuthorityEnded) => CleanupPhase::Released,
        (phase, _) => phase,
    }
}

/// The durable attachment of one Start or Resume transport and its [`CleanupPhase`].
pub(super) struct AttachmentCleanup {
    attachment: Option<StreamSession>,
    phase: CleanupPhase,
}

impl AttachmentCleanup {
    /// A transport that holds `attachment`; without an attachment, nothing is held.
    pub(super) fn new(attachment: Option<StreamSession>) -> Self {
        let phase = if attachment.is_some() {
            CleanupPhase::Held
        } else {
            CleanupPhase::Released
        };
        Self { attachment, phase }
    }

    pub(super) fn observe(&mut self, event: CleanupEvent) {
        self.phase = cleanup_transition(self.phase, event);
    }

    pub(super) fn observe_error(&mut self, error: &SessionError) {
        self.observe(CleanupEvent::of_session_error(error));
    }

    #[cfg(test)]
    pub(super) fn phase(&self) -> CleanupPhase {
        self.phase
    }

    /// Writes the detach only while the attachment is held.
    pub(super) async fn finish(self) {
        detach_durable_attachment(self.attachment.filter(|_| self.phase == CleanupPhase::Held))
            .await;
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
        SessionError::Fenced(_) | SessionError::IncompleteOutput(_) => false,
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
    cleanup: &mut AttachmentCleanup,
    error: SessionError,
    drain_outputs: impl Future<Output = Result<(), SessionError>>,
) {
    cleanup.observe_error(&error);
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
        cleanup.observe_error(&finish_error);
        send_protocol_failure(responses, finish_error).await;
        return;
    }
    if let Err(cleanup_error) = streams
        .pump_input_cancellations(responses, high_waters)
        .await
    {
        cleanup.observe_error(&cleanup_error);
    }
    if let Err(drain_error) = drain_outputs.await {
        cleanup.observe_error(&drain_error);
    }
    send_protocol_failure(responses, error).await;
}

/// Joins every output pump. The first read that failed for a time is returned.
pub(super) async fn join_output_pumps(
    pumps: &mut tokio::task::JoinSet<Result<(), SessionError>>,
) -> Result<(), SessionError> {
    futures::stream::poll_fn(|context| pumps.poll_join_next(context))
        .fold(Ok(()), |joined, pumped| async move {
            match (joined, pumped) {
                (Err(error), _) => Err(error),
                (_, Ok(Err(SessionError::Unreadable(unreadable)))) if unreadable.is_transient() => {
                    Err(SessionError::Unreadable(unreadable))
                }
                (joined, _) => joined,
            }
        })
        .await
}
