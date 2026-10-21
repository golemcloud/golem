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

pub(super) fn card_id_to_wit(card_id: CardId) -> oplog::CardId {
    oplog::CardId {
        uuid: card_id.0.into(),
    }
}

pub(super) fn card_id_from_wit(card_id: oplog::CardId) -> CardId {
    CardId(card_id.uuid.into())
}

fn wallet_version_token_to_wit(value: WalletVersionToken) -> oplog::WalletVersionToken {
    oplog::WalletVersionToken {
        wallet_id_hash: value.wallet_id_hash.to_vec(),
        generation: value.generation,
    }
}

fn wallet_version_token_from_wit(
    value: oplog::WalletVersionToken,
) -> Result<WalletVersionToken, String> {
    Ok(WalletVersionToken {
        wallet_id_hash: value.wallet_id_hash.try_into().map_err(|value: Vec<u8>| {
            format!("wallet ID hash must contain 32 bytes, got {}", value.len())
        })?,
        generation: value.generation,
    })
}

pub(super) fn invocation_wallet_pin_to_wit(
    value: InvocationWalletPin,
) -> oplog::RawInvocationWalletPin {
    oplog::RawInvocationWalletPin {
        wallet_token: wallet_version_token_to_wit(value.wallet_token),
        pinned_card_ids: value
            .pinned_card_ids
            .into_iter()
            .map(card_id_to_wit)
            .collect(),
        scope_card_id: value.scope_card_id.map(card_id_to_wit),
    }
}

pub(super) fn invocation_wallet_pin_from_wit(
    value: oplog::RawInvocationWalletPin,
) -> Result<InvocationWalletPin, String> {
    Ok(InvocationWalletPin {
        wallet_token: wallet_version_token_from_wit(value.wallet_token)?,
        pinned_card_ids: value
            .pinned_card_ids
            .into_iter()
            .map(card_id_from_wit)
            .collect(),
        scope_card_id: value.scope_card_id.map(card_id_from_wit),
    })
}

pub(super) fn public_invocation_wallet_pin_to_wit(
    value: PublicInvocationWalletPin,
) -> oplog::PublicInvocationWalletPin {
    oplog::PublicInvocationWalletPin {
        wallet_token: wallet_version_token_to_wit(value.wallet_token),
        scope_card_id: value.scope_card_id.map(card_id_to_wit),
    }
}

pub(super) fn queued_card_event_to_wit(
    value: PublicQueuedCardEvent,
) -> Result<oplog::QueuedCardEvent, String> {
    match value {
        PublicQueuedCardEvent::Install(event) => Ok(oplog::QueuedCardEvent::Install(
            oplog::QueuedCardEventInstall {
                card_id: card_id_to_wit(event.card_id),
            },
        )),
        PublicQueuedCardEvent::Revoke(event) => Ok(oplog::QueuedCardEvent::Revoke(
            oplog::QueuedCardEventRevoke {
                card_id: card_id_to_wit(event.card_id),
            },
        )),
        PublicQueuedCardEvent::TransferStarted(_) => {
            Err("card transfer events are not representable in golem:api/oplog@1.5.0".to_string())
        }
        PublicQueuedCardEvent::TransferReceived(_) => Err(
            "received card transfer events are not representable in golem:api/oplog@1.5.0"
                .to_string(),
        ),
    }
}

pub(super) fn raw_queued_card_event_to_wit(
    value: QueuedCardEvent,
) -> Result<oplog::QueuedCardEvent, String> {
    match value {
        QueuedCardEvent::Install(event) => Ok(oplog::QueuedCardEvent::Install(
            oplog::QueuedCardEventInstall {
                card_id: card_id_to_wit(event.card_id),
            },
        )),
        QueuedCardEvent::Revoke(event) => Ok(oplog::QueuedCardEvent::Revoke(
            oplog::QueuedCardEventRevoke {
                card_id: card_id_to_wit(event.card_id),
            },
        )),
        QueuedCardEvent::TransferStarted(_) => {
            Err("card transfer events are not representable in golem:api/oplog@1.5.0".to_string())
        }
        QueuedCardEvent::TransferReceived(_) => Err(
            "received card transfer events are not representable in golem:api/oplog@1.5.0"
                .to_string(),
        ),
    }
}

pub(super) fn raw_queued_card_event_from_wit(
    value: oplog::QueuedCardEvent,
) -> Result<QueuedCardEvent, String> {
    match value {
        oplog::QueuedCardEvent::Install(event) => {
            Ok(QueuedCardEvent::Install(QueuedCardEventCard {
                card_id: card_id_from_wit(event.card_id),
                card: None,
            }))
        }
        oplog::QueuedCardEvent::Revoke(event) => Ok(QueuedCardEvent::Revoke(QueuedCardEventCard {
            card_id: card_id_from_wit(event.card_id),
            card: None,
        })),
    }
}

pub(super) fn card_install_failure_to_wit(value: CardInstallFailure) -> oplog::CardInstallFailure {
    match value {
        CardInstallFailure::CardRevoked => oplog::CardInstallFailure::CardRevoked,
        CardInstallFailure::NotFound => oplog::CardInstallFailure::NotFound,
        CardInstallFailure::RecipientMismatch => oplog::CardInstallFailure::RecipientMismatch,
        CardInstallFailure::NotPermitted => oplog::CardInstallFailure::NotPermitted,
    }
}

pub(super) fn card_install_failure_from_wit(
    value: oplog::CardInstallFailure,
) -> CardInstallFailure {
    match value {
        oplog::CardInstallFailure::CardRevoked => CardInstallFailure::CardRevoked,
        oplog::CardInstallFailure::NotFound => CardInstallFailure::NotFound,
        oplog::CardInstallFailure::RecipientMismatch => CardInstallFailure::RecipientMismatch,
        oplog::CardInstallFailure::NotPermitted => CardInstallFailure::NotPermitted,
    }
}
