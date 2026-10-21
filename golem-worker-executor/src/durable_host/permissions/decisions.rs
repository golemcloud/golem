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
    BinaryDeserializer, BinarySerializer, DurableCallSession, DurableFunctionType,
    DurableWorkerCtx, HostPayloadPair, HostRequestPermissionCardDecision,
    HostResponsePermissionCardDecision, NotCancellable, PermissionCardDeriveError,
    SerializablePermissionError, Uuid, WorkerCtx, WorkerExecutorError, anyhow, deserialize,
    permissions_types, serialize,
};

pub(super) fn permission_error_to_anyhow(
    error: permissions_types::PermissionError,
) -> anyhow::Error {
    anyhow!("permission-card error: {error:?}")
}

pub(super) fn worker_error_to_permission_error(
    error: WorkerExecutorError,
) -> permissions_types::PermissionError {
    permissions_types::PermissionError::NotPermitted(format!(
        "permission-card store lookup failed: {error}"
    ))
}

pub(super) fn permission_error_to_serializable(
    error: permissions_types::PermissionError,
) -> SerializablePermissionError {
    use permissions_types::PermissionError as E;
    match error {
        E::NotPermitted(message) => SerializablePermissionError::NotPermitted(message),
        E::LowerBoundTooBroad(message) => SerializablePermissionError::LowerBoundTooBroad(message),
        E::UpperBoundTooBroad(message) => SerializablePermissionError::UpperBoundTooBroad(message),
        E::WalletMultiSourceRequired(message) => {
            SerializablePermissionError::WalletMultiSourceRequired(message)
        }
        E::CardRevoked(message) => SerializablePermissionError::CardRevoked(message),
        E::CardExpired(message) => SerializablePermissionError::CardExpired(message),
        E::InstallRecipientMismatch(message) => {
            SerializablePermissionError::InstallRecipientMismatch(message)
        }
        E::UnknownResourceClass(message) => {
            SerializablePermissionError::UnknownResourceClass(message)
        }
        E::InvalidPattern(message) => SerializablePermissionError::InvalidPattern(message),
        E::InvalidVerb(message) => SerializablePermissionError::InvalidVerb(message),
        E::InvalidOwner(message) => SerializablePermissionError::InvalidOwner(message),
        E::InvalidRecipient(message) => SerializablePermissionError::InvalidRecipient(message),
    }
}

pub(super) fn permission_error_from_serializable(
    error: SerializablePermissionError,
) -> permissions_types::PermissionError {
    use permissions_types::PermissionError as E;
    match error {
        SerializablePermissionError::NotPermitted(message) => E::NotPermitted(message),
        SerializablePermissionError::LowerBoundTooBroad(message) => E::LowerBoundTooBroad(message),
        SerializablePermissionError::UpperBoundTooBroad(message) => E::UpperBoundTooBroad(message),
        SerializablePermissionError::WalletMultiSourceRequired(message) => {
            E::WalletMultiSourceRequired(message)
        }
        SerializablePermissionError::CardRevoked(message) => E::CardRevoked(message),
        SerializablePermissionError::CardExpired(message) => E::CardExpired(message),
        SerializablePermissionError::InstallRecipientMismatch(message) => {
            E::InstallRecipientMismatch(message)
        }
        SerializablePermissionError::UnknownResourceClass(message) => {
            E::UnknownResourceClass(message)
        }
        SerializablePermissionError::InvalidPattern(message) => E::InvalidPattern(message),
        SerializablePermissionError::InvalidVerb(message) => E::InvalidVerb(message),
        SerializablePermissionError::InvalidOwner(message) => E::InvalidOwner(message),
        SerializablePermissionError::InvalidRecipient(message) => E::InvalidRecipient(message),
    }
}

/// Makes the authorization decision of a card operation in a `ReadLocal` durable call that
/// records it, allowed or denied, together with the data the operation continues with.
///
/// Card events are hints. Replay publishes every card event up to the next recorded entry when it
/// commits a recorded entry, and the next authority boundary applies them, so a decision that
/// records nothing can observe on replay a card event that live execution applied only to a later
/// operation. Replay returns the recorded decision instead of deciding again; the repair of an
/// incomplete decision decides at its own `Start`.
pub(super) async fn record_card_decision<Ctx, Pair, T, Decide>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    card_ids: Vec<Uuid>,
    decide: Decide,
) -> anyhow::Result<Result<T, permissions_types::PermissionError>>
where
    Ctx: WorkerCtx,
    Pair: HostPayloadPair<
            Req = HostRequestPermissionCardDecision,
            Resp = HostResponsePermissionCardDecision,
        >,
    T: BinarySerializer + BinaryDeserializer,
    Decide: AsyncFnOnce(
        &mut DurableWorkerCtx<Ctx>,
    ) -> anyhow::Result<Result<T, permissions_types::PermissionError>>,
{
    let response = DurableCallSession::<Pair, NotCancellable>::invoke(
        ctx,
        HostRequestPermissionCardDecision { card_ids },
        DurableFunctionType::ReadLocal,
        async move |ctx| {
            let result = match decide(ctx).await? {
                Ok(decided) => Ok(serialize(&decided).map_err(|error| {
                    anyhow!("failed to serialize a permission-card decision: {error}")
                })?),
                Err(error) => Err(permission_error_to_serializable(error)),
            };
            Ok::<_, anyhow::Error>(HostResponsePermissionCardDecision { result })
        },
    )
    .await?;
    match response.result {
        Ok(decided) => Ok(Ok(deserialize(&decided).map_err(|error| {
            anyhow!("failed to deserialize a recorded permission-card decision: {error}")
        })?)),
        Err(error) => Ok(Err(permission_error_from_serializable(error))),
    }
}

pub(super) fn permission_error_to_derive_error(
    error: permissions_types::PermissionError,
) -> PermissionCardDeriveError {
    match error {
        permissions_types::PermissionError::CardRevoked(message) => {
            PermissionCardDeriveError::CardRevoked(message)
        }
        permissions_types::PermissionError::CardExpired(message) => {
            PermissionCardDeriveError::CardExpired(message)
        }
        permissions_types::PermissionError::NotPermitted(message) => {
            PermissionCardDeriveError::NotPermitted(message)
        }
        error => PermissionCardDeriveError::NotPermitted(format!("{error:?}")),
    }
}

pub(super) fn derive_error_to_permission_error(
    error: PermissionCardDeriveError,
) -> permissions_types::PermissionError {
    match error {
        PermissionCardDeriveError::CardRevoked(message) => {
            permissions_types::PermissionError::CardRevoked(message)
        }
        PermissionCardDeriveError::CardExpired(message) => {
            permissions_types::PermissionError::CardExpired(message)
        }
        PermissionCardDeriveError::NotPermitted(message) => {
            permissions_types::PermissionError::NotPermitted(message)
        }
    }
}
