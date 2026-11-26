///|
/// The handle of an accepted invocation of an agent method, returned by the
/// `trigger_*` and `schedule_*` methods of a generated client, and rebuilt
/// from its ID by the generated `get_*_invocation`.
///
/// The ID only locates the invocation; it is not authority. Only the caller
/// that started the invocation, or a caller with the applicable management
/// permission, may observe or cancel it: the target authorizes every
/// operation, and denies the others with `RpcError::Denied`.
///
/// Dropping the handle, or cancelling a task that waits in `await_result`,
/// only detaches the caller: the invocation keeps running. Trigger and
/// schedule refuse a method that takes or returns a stream, and never lend a
/// scope card. A running invocation cannot be cancelled: `cancel` raises
/// `RpcError::CannotCancelRunning`.
pub struct Invocation[T] {
  priv id : InvocationId
  priv raw : @agentHost.Invocation
  priv decode : (@types.SchemaValueTree?) -> T raise
}

///|
/// The handle of an invocation resource, whose result `decode` reads. The
/// handle owns the resource.
pub fn[T] Invocation::from_raw(
  raw : @agentHost.Invocation,
  decode : (@types.SchemaValueTree?) -> T raise,
) -> Invocation[T] {
  { id: InvocationId::from_wit(raw.id()), raw, decode, }
}

///|
/// Rebuilds the handle of the invocation with the given ID, whose result
/// `decode` reads. Makes no remote call.
pub fn[T] Invocation::get(
  id : InvocationId,
  decode : (@types.SchemaValueTree?) -> T raise,
) -> Invocation[T] {
  { id, raw: @agentHost.Invocation::get(id.to_wit()), decode, }
}

///|
/// The ID of the invocation. Store it to reach the invocation from a later
/// invocation of this agent.
pub fn[T] Invocation::id(self : Invocation[T]) -> InvocationId {
  self.id
}

///|
/// The current lifecycle state of the invocation. Raises
/// `RpcError::NotFound` when the invocation or its agent no longer exists.
pub async fn[T] Invocation::status(self : Invocation[T]) -> InvocationStatus {
  match self.raw.status() {
    Ok(status) => InvocationStatus::from_host(status)
    Err(error) => raise error
  }
}

///|
/// Awaits the result of the invocation, without starting, resuming or
/// resubmitting it. Raises `RpcError::Cancelled` when the invocation was
/// cancelled before it started, and the error of `decode` when the result does
/// not decode. Every call asks the host again.
pub async fn[T] Invocation::await_result(self : Invocation[T]) -> T {
  match self.raw.await_() {
    Ok(output) => (self.decode)(output)
    Err(error) => raise error
  }
}

///|
/// Withdraws the invocation if it has not started yet, and returns `true`.
/// Returns `false` when the invocation already finished or was already
/// cancelled, and raises `RpcError::CannotCancelRunning` when it is running.
pub async fn[T] Invocation::cancel(self : Invocation[T]) -> Bool {
  match self.raw.cancel() {
    Ok(withdrawn) => withdrawn
    Err(error) => raise error
  }
}

///|
/// Releases the handle. The invocation keeps running.
pub fn[T] Invocation::drop(self : Invocation[T]) -> Unit {
  self.raw.drop()
}

///|
/// The lifecycle state of an accepted invocation.
pub(all) enum InvocationStatus {
  /// Accepted and waiting for its turn
  Pending
  /// Accepted and waiting for its scheduled time
  Scheduled
  /// Started and executing
  Running
  /// Started and waiting, for example for a promise or a timer
  Waiting
  /// Started and waiting to retry after a failure
  Retrying
  /// Started, and its agent was interrupted by an operator
  Interrupted
  /// Finished with a result
  Completed
  /// Finished with a failure
  Failed
  /// Withdrawn before it started
  Cancelled
  /// Recorded its result, then its agent failed during the invocation's
  /// background work
  BackgroundFailed
} derive(Debug, Eq)

///|
pub extend InvocationStatus with Debug::{to_repr}

///|
pub extend InvocationStatus with Eq::{equal, not_equal}

///|
fn InvocationStatus::from_host(
  status : @agentHost.InvocationStatus,
) -> InvocationStatus {
  match status {
    PENDING => Pending
    SCHEDULED => Scheduled
    RUNNING => Running
    WAITING => Waiting
    RETRYING => Retrying
    INTERRUPTED => Interrupted
    COMPLETED => Completed
    FAILED => Failed
    CANCELLED => Cancelled
    BACKGROUND_FAILED => BackgroundFailed
  }
}
