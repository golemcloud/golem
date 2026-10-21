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

use super::*;

pub(super) fn filesystem_interrupt<Ctx: WorkerCtx, U: Send + 'static>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
) -> FilesystemInterruptSignal {
    accessor.with(|mut access| {
        durable_worker_ctx::<Ctx, U>(access.data_mut()).create_interrupt_signal()
    })
}

pub(super) async fn observe_p3_filesystem<T>(
    interrupt: FilesystemInterruptSignal,
    operation: impl Future<Output = FilesystemResult<T>>,
) -> FilesystemResult<T> {
    observe_filesystem_operation(interrupt, operation)
        .await
        .map_err(FilesystemError::trap)?
}

/// Checks the filesystem permissions of a protected guest import in a durable call, so that
/// replay returns the recorded decision instead of checking again. It reserves the invocation id
/// of the import, so it must run before the import's first `await`.
pub(super) async fn authorize_paths<Ctx: WorkerCtx, U: Send + 'static>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    requests: &[(FilesystemVerb, CanonicalGuestPath)],
) -> FilesystemResult<Option<LiveAuthorizationPermit>> {
    reserve_invocation_id_before_await(accessor, durable_worker_ctx::<Ctx, U>)
        .map_err(|error| FilesystemError::trap(wasmtime::Error::from(error)))?;
    let (response, permit) = run_permission_checked_access::<Ctx, U, FilesystemPermissionCheck>(
        accessor,
        permission_check_request(requests),
        requests,
        async || -> wasmtime::Result<_> { Ok(HostResponsePermissionCheck { result: Ok(()) }) },
    )
    .await
    .map_err(FilesystemError::trap)?;
    match response.result {
        Ok(()) => Ok(permit),
        Err(_) => Err(types::ErrorCode::NotPermitted.into()),
    }
}

/// Constructs the targets of a filesystem permission check without making a live decision.
fn p3_permission_targets<Ctx: WorkerCtx, U: 'static>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    requests: &[(FilesystemVerb, CanonicalGuestPath)],
) -> Result<Vec<PermissionTarget>, String> {
    Ok(accessor.with(|mut access| {
        let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
        filesystem_permission_targets(ctx, requests)
    }))
}

/// Runs a `ReadLocal` durable call that the filesystem permissions of `requests` can deny, and
/// returns its response with the permit of an allowed live decision.
pub(super) async fn run_permission_checked_access<Ctx, U, Pair>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    request: Pair::Req,
    requests: &[(FilesystemVerb, CanonicalGuestPath)],
    live: impl AsyncFnOnce() -> wasmtime::Result<Pair::Resp>,
) -> wasmtime::Result<(Pair::Resp, Option<LiveAuthorizationPermit>)>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
    Pair: DeniablePair,
    Pair::Req: Clone,
{
    run_deniable_filesystem_access::<Ctx, U, Pair, Cancellable>(
        accessor,
        request,
        p3_permission_targets(accessor, requests),
        live,
    )
    .await
}

/// Runs a `ReadLocal` durable call checked against `targets`, and returns its response with the
/// permit of an allowed new decision.
///
/// A live call decides before it writes its `Start`. A denied call records the denial in the
/// `Start` and completes with [`DeniablePair::denied`] without running `live`, so the recovery of
/// an incomplete denied `Start` gives the same response. A replayed call uses the recorded
/// decision: an incomplete `Start` without a denial was allowed, so its repair runs `live` without
/// a new decision. A new call decides after its claim resolves and before it writes its `Start`.
async fn run_deniable_filesystem_access<Ctx, U, Pair, P>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    request: Pair::Req,
    targets: Result<Vec<PermissionTarget>, String>,
    live: impl AsyncFnOnce() -> wasmtime::Result<Pair::Resp>,
) -> wasmtime::Result<(Pair::Resp, Option<LiveAuthorizationPermit>)>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
    Pair: DeniablePair,
    Pair::Req: Clone,
    P: DropPolicy,
{
    let (call, decision) = DurableCallSession::<Pair, P>::start_authorized_access(
        accessor,
        durable_worker_ctx::<Ctx, U>,
        DurableFunctionType::ReadLocal,
        AccessClaimOptions::default(),
        request.clone(),
        targets,
    )
    .await?;
    let (mut permit, denied) = match decision {
        Some(Ok(permit)) => (Some(permit), None),
        Some(Err(denial)) => (None, Some(Pair::denied(&request, &denial))),
        None => (None, None),
    };
    let response = if call.is_live() {
        match denied {
            Some(denied) => call
                .complete_access(accessor, durable_worker_ctx::<Ctx, U>, denied)
                .await
                .map_err(|error| wasmtime::Error::from_anyhow(error.into_marked_anyhow()))?,
            None => {
                call.run_live_action_access(accessor, durable_worker_ctx::<Ctx, U>, live)
                    .await?
            }
        }
    } else {
        permit = None;
        match call
            .replay_access(accessor, durable_worker_ctx::<Ctx, U>)
            .await?
        {
            CallReplayOutcome::Replayed(response) => response,
            CallReplayOutcome::Incomplete(call) => {
                call.run_live_action_access(accessor, durable_worker_ctx::<Ctx, U>, live)
                    .await?
            }
        }
    };
    Ok((response, permit))
}

fn deserialize_write_admission(
    result: Result<(), SerializableP3FileSystemError>,
) -> wasmtime::Result<Result<(), types::ErrorCode>> {
    match result {
        Ok(()) => Ok(Ok(())),
        Err(SerializableP3FileSystemError::ErrorCode(error)) => {
            Ok(Err(types::ErrorCode::from(error)))
        }
        Err(SerializableP3FileSystemError::Generic(error)) => Err(wasmtime::Error::msg(error)),
    }
}

pub(super) async fn admit_stream_write<Pair, Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    fd: &Resource<Descriptor>,
) -> wasmtime::Result<Result<Option<LiveAuthorizationPermit>, types::ErrorCode>>
where
    Pair: DeniablePair<Req = HostRequestNoInput, Resp = HostResponseP3FileSystemWriteAdmission>,
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    reserve_invocation_id_before_await(accessor, durable_worker_ctx::<Ctx, U>)
        .map_err(wasmtime::Error::from)?;
    let fd_rep = fd.rep();
    let (response, authorization_permit) =
        run_deniable_filesystem_access::<Ctx, U, Pair, NotCancellable>(
            accessor,
            HostRequestNoInput {},
            check_stream_write::<Ctx, U>(accessor, fd_rep),
            async || -> wasmtime::Result<_> {
                Ok(HostResponseP3FileSystemWriteAdmission { result: Ok(()) })
            },
        )
        .await?;

    Ok(deserialize_write_admission(response.result)?.map(|()| authorization_permit))
}

/// Constructs the targets of a stream write; a descriptor without a guest path denies the write.
fn check_stream_write<Ctx: WorkerCtx, U: 'static>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    fd_rep: u32,
) -> Result<Vec<PermissionTarget>, String> {
    match descriptor_guest_path_from_accessor::<Ctx, U>(accessor, &Resource::new_borrow(fd_rep), "")
    {
        Ok(path) => p3_permission_targets(accessor, &[(FilesystemVerb::Write, path)]),
        Err(_) => Err("the descriptor has no guest path".to_string()),
    }
}
