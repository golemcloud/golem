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

use crate::base_model::oplog::{
    CardInstallFailure, PublicQueuedCardEvent, PublicQueuedCardEventTransfer,
    PublicQueuedCardEventTransferReceived, QueuedCardEvent, QueuedCardEventCard,
    QueuedCardEventTransfer, QueuedCardEventTransferReceived,
};
use crate::model::card::{
    AccountCardHolder, AgentCardHolder, ApplicationCardHolder, CardHolder, CardId,
    InvocationWalletPin, PublicInvocationWalletPin, WalletVersionToken,
};
use crate::model::oplog::PublicQueuedCardEventCard;

pub(super) fn card_holder_from_proto(
    value: golem_api_grpc::proto::golem::worker::CardHolder,
) -> Result<CardHolder, String> {
    use golem_api_grpc::proto::golem::worker::card_holder::Holder;

    match value.holder.ok_or("Missing card holder")? {
        Holder::Account(account_id) => Ok(CardHolder::Account(AccountCardHolder {
            account_id: account_id.into(),
        })),
        Holder::Application(application_id) => Ok(CardHolder::Application(ApplicationCardHolder {
            application_id: application_id.into(),
        })),
        Holder::Agent(agent_id) => Ok(CardHolder::Agent(AgentCardHolder {
            agent_id: agent_id.try_into()?,
        })),
    }
}

pub(super) fn card_holder_to_proto(
    value: CardHolder,
) -> golem_api_grpc::proto::golem::worker::CardHolder {
    use golem_api_grpc::proto::golem::worker::card_holder::Holder;

    let holder = match value {
        CardHolder::Account(holder) => Holder::Account(holder.account_id.into()),
        CardHolder::Application(holder) => Holder::Application(holder.application_id.into()),
        CardHolder::Agent(holder) => Holder::Agent(holder.agent_id.into()),
    };
    golem_api_grpc::proto::golem::worker::CardHolder {
        holder: Some(holder),
    }
}

fn wallet_version_token_from_proto(
    value: golem_api_grpc::proto::golem::worker::WalletVersionToken,
) -> Result<WalletVersionToken, String> {
    let wallet_id_hash = value
        .wallet_id_hash
        .try_into()
        .map_err(|hash: Vec<u8>| format!("wallet_id_hash must be 32 bytes, got {}", hash.len()))?;
    Ok(WalletVersionToken {
        wallet_id_hash,
        generation: value.generation,
    })
}

fn wallet_version_token_to_proto(
    value: WalletVersionToken,
) -> golem_api_grpc::proto::golem::worker::WalletVersionToken {
    golem_api_grpc::proto::golem::worker::WalletVersionToken {
        wallet_id_hash: value.wallet_id_hash.to_vec(),
        generation: value.generation,
    }
}

pub(super) fn invocation_wallet_pin_from_proto(
    value: golem_api_grpc::proto::golem::worker::RawInvocationWalletPin,
) -> Result<InvocationWalletPin, String> {
    Ok(InvocationWalletPin {
        wallet_token: wallet_version_token_from_proto(
            value.wallet_token.ok_or("Missing wallet_token field")?,
        )?,
        pinned_card_ids: value
            .pinned_card_ids
            .into_iter()
            .map(|card_id| CardId(card_id.into()))
            .collect(),
        scope_card_id: value.scope_card_id.map(|card_id| CardId(card_id.into())),
    })
}

pub(super) fn invocation_wallet_pin_to_proto(
    value: InvocationWalletPin,
) -> golem_api_grpc::proto::golem::worker::RawInvocationWalletPin {
    golem_api_grpc::proto::golem::worker::RawInvocationWalletPin {
        wallet_token: Some(wallet_version_token_to_proto(value.wallet_token)),
        pinned_card_ids: value
            .pinned_card_ids
            .into_iter()
            .map(|card_id| card_id.0.into())
            .collect(),
        scope_card_id: value.scope_card_id.map(|card_id| card_id.0.into()),
    }
}

pub(super) fn public_invocation_wallet_pin_from_proto(
    value: golem_api_grpc::proto::golem::worker::PublicInvocationWalletPin,
) -> Result<PublicInvocationWalletPin, String> {
    Ok(PublicInvocationWalletPin {
        wallet_token: wallet_version_token_from_proto(
            value.wallet_token.ok_or("Missing wallet_token field")?,
        )?,
        scope_card_id: value.scope_card_id.map(|card_id| CardId(card_id.into())),
    })
}

pub(super) fn public_invocation_wallet_pin_to_proto(
    value: PublicInvocationWalletPin,
) -> golem_api_grpc::proto::golem::worker::PublicInvocationWalletPin {
    golem_api_grpc::proto::golem::worker::PublicInvocationWalletPin {
        wallet_token: Some(wallet_version_token_to_proto(value.wallet_token)),
        scope_card_id: value.scope_card_id.map(|card_id| card_id.0.into()),
    }
}

pub(super) fn public_queued_card_event_from_proto(
    value: golem_api_grpc::proto::golem::worker::QueuedCardEvent,
) -> Result<PublicQueuedCardEvent, String> {
    use golem_api_grpc::proto::golem::worker::queued_card_event::Event;

    match value.event.ok_or("Missing queued card event")? {
        Event::Install(event) => Ok(PublicQueuedCardEvent::Install(PublicQueuedCardEventCard {
            card_id: CardId(event.card_id.ok_or("Missing card_id")?.into()),
        })),
        Event::Revoke(event) => Ok(PublicQueuedCardEvent::Revoke(PublicQueuedCardEventCard {
            card_id: CardId(event.card_id.ok_or("Missing card_id")?.into()),
        })),
        Event::TransferStarted(event) => Ok(PublicQueuedCardEvent::TransferStarted(
            PublicQueuedCardEventTransfer {
                transfer_id: event.transfer_id.ok_or("Missing transfer_id")?.into(),
                card_id: CardId(event.card_id.ok_or("Missing card_id")?.into()),
                target_holder: card_holder_from_proto(
                    event.target_holder.ok_or("Missing target_holder")?,
                )?,
            },
        )),
        Event::TransferReceived(event) => Ok(PublicQueuedCardEvent::TransferReceived(
            PublicQueuedCardEventTransferReceived {
                transfer_id: event.transfer_id.ok_or("Missing transfer_id")?.into(),
                card_id: CardId(event.card_id.ok_or("Missing card_id")?.into()),
            },
        )),
    }
}

pub(super) fn public_queued_card_event_to_proto(
    value: PublicQueuedCardEvent,
) -> golem_api_grpc::proto::golem::worker::QueuedCardEvent {
    use golem_api_grpc::proto::golem::worker::queued_card_event as proto;

    let event = match value {
        PublicQueuedCardEvent::Install(event) => proto::Event::Install(proto::Install {
            card_id: Some(event.card_id.0.into()),
        }),
        PublicQueuedCardEvent::Revoke(event) => proto::Event::Revoke(proto::Revoke {
            card_id: Some(event.card_id.0.into()),
        }),
        PublicQueuedCardEvent::TransferStarted(event) => {
            proto::Event::TransferStarted(proto::TransferStarted {
                transfer_id: Some(event.transfer_id.into()),
                card_id: Some(event.card_id.0.into()),
                target_holder: Some(card_holder_to_proto(event.target_holder)),
            })
        }
        PublicQueuedCardEvent::TransferReceived(event) => {
            proto::Event::TransferReceived(proto::TransferReceived {
                transfer_id: Some(event.transfer_id.into()),
                card_id: Some(event.card_id.0.into()),
            })
        }
    };

    golem_api_grpc::proto::golem::worker::QueuedCardEvent { event: Some(event) }
}

pub(super) fn raw_queued_card_event_from_proto(
    value: golem_api_grpc::proto::golem::worker::RawQueuedCardEvent,
) -> Result<QueuedCardEvent, String> {
    use golem_api_grpc::proto::golem::worker::raw_queued_card_event::Event;

    match value.event.ok_or("Missing queued card event")? {
        Event::Install(event) => {
            let card_id = CardId(event.card_id.ok_or("Missing card_id")?.into());
            if event.card.is_empty() {
                return Err("Queued card install is missing card payload".to_string());
            }
            let card = deserialize_stored_card(&event.card, "queued card install")?;
            if card.card_id() != card_id {
                return Err("Queued card install card payload does not match card_id".to_string());
            }
            Ok(QueuedCardEvent::Install(QueuedCardEventCard {
                card_id,
                card: Some(card),
            }))
        }
        Event::Revoke(event) => Ok(QueuedCardEvent::Revoke(QueuedCardEventCard {
            card_id: CardId(event.card_id.ok_or("Missing card_id")?.into()),
            card: None,
        })),
        Event::TransferStarted(event) => {
            let card_id = CardId(event.card_id.ok_or("Missing card_id")?.into());
            if event.card.is_empty() {
                return Err("Queued card transfer is missing card payload".to_string());
            }
            let card = deserialize_stored_card(&event.card, "queued card transfer")?;
            Ok(QueuedCardEvent::TransferStarted(QueuedCardEventTransfer {
                transfer_id: event.transfer_id.ok_or("Missing transfer_id")?.into(),
                card_id,
                card: Some(card),
                target_holder: card_holder_from_proto(
                    event.target_holder.ok_or("Missing target_holder")?,
                )?,
            }))
        }
        Event::TransferReceived(event) => {
            let card_id = CardId(event.card_id.ok_or("Missing card_id")?.into());
            if event.card.is_empty() {
                return Err("Received card transfer is missing card payload".to_string());
            }
            let card = deserialize_stored_card(&event.card, "received card transfer")?;
            if card.card_id() != card_id {
                return Err("Received card transfer payload does not match card_id".to_string());
            }
            Ok(QueuedCardEvent::TransferReceived(
                QueuedCardEventTransferReceived {
                    transfer_id: event.transfer_id.ok_or("Missing transfer_id")?.into(),
                    source_card_id: CardId(
                        event.source_card_id.ok_or("Missing source_card_id")?.into(),
                    ),
                    card_id,
                    card: Some(card),
                },
            ))
        }
    }
}

pub(super) fn deserialize_stored_card(
    bytes: &[u8],
    description: &str,
) -> Result<crate::model::card::StoredCard, String> {
    let version = bytes
        .first()
        .ok_or_else(|| format!("Failed to deserialize {description}: missing payload"))?;
    if *version != crate::serialization::SERIALIZATION_VERSION_V3 {
        return Err(format!(
            "Failed to deserialize {description}: invalid serialization version: {version}"
        ));
    }
    std::panic::catch_unwind(|| {
        let mut context =
            desert_rust::DeserializationContext::new(&bytes[1..], desert_rust::Options::default());
        let card =
            <crate::model::card::StoredCard as desert_rust::BinaryDeserializer>::deserialize(
                &mut context,
            )?;
        match desert_rust::BinaryInput::read_u8(&mut context) {
            Err(desert_rust::Error::InputEndedUnexpectedly) => Ok(card),
            Ok(_) => Err(desert_rust::Error::DeserializationFailure(
                "trailing bytes after StoredCard payload".to_string(),
            )),
            Err(error) => Err(error),
        }
    })
    .map_err(|_| format!("Failed to deserialize {description}: malformed payload"))?
    .map_err(|err| format!("Failed to deserialize {description}: {err}"))
}

pub(super) fn raw_queued_card_event_to_proto(
    value: QueuedCardEvent,
) -> Result<golem_api_grpc::proto::golem::worker::RawQueuedCardEvent, String> {
    use golem_api_grpc::proto::golem::worker::raw_queued_card_event as proto;

    let event = match value {
        QueuedCardEvent::Install(event) => {
            let card = event
                .card
                .ok_or("Queued card install is missing card payload")?;
            if card.card_id() != event.card_id {
                return Err("Queued card install card payload does not match card_id".to_string());
            }
            proto::Event::Install(proto::Install {
                card_id: Some(event.card_id.0.into()),
                card: crate::serialization::serialize(&card)?,
            })
        }
        QueuedCardEvent::Revoke(event) => proto::Event::Revoke(proto::Revoke {
            card_id: Some(event.card_id.0.into()),
        }),
        QueuedCardEvent::TransferStarted(event) => {
            let card = event
                .card
                .ok_or("Queued card transfer is missing card payload")?;
            proto::Event::TransferStarted(proto::TransferStarted {
                transfer_id: Some(event.transfer_id.into()),
                card_id: Some(event.card_id.0.into()),
                card: crate::serialization::serialize(&card)?,
                target_holder: Some(card_holder_to_proto(event.target_holder)),
            })
        }
        QueuedCardEvent::TransferReceived(event) => {
            let card = event
                .card
                .ok_or("Received card transfer is missing card payload")?;
            if card.card_id() != event.card_id {
                return Err("Received card transfer payload does not match card_id".to_string());
            }
            proto::Event::TransferReceived(proto::TransferReceived {
                transfer_id: Some(event.transfer_id.into()),
                card_id: Some(event.card_id.0.into()),
                card: crate::serialization::serialize(&card)?,
                source_card_id: Some(event.source_card_id.0.into()),
            })
        }
    };

    Ok(golem_api_grpc::proto::golem::worker::RawQueuedCardEvent { event: Some(event) })
}

pub(super) fn card_install_failure_from_proto(
    value: golem_api_grpc::proto::golem::worker::CardInstallFailure,
) -> Result<CardInstallFailure, String> {
    match value {
        golem_api_grpc::proto::golem::worker::CardInstallFailure::Unspecified => {
            Err("Unspecified card install failure".to_string())
        }
        golem_api_grpc::proto::golem::worker::CardInstallFailure::CardRevoked => {
            Ok(CardInstallFailure::CardRevoked)
        }
        golem_api_grpc::proto::golem::worker::CardInstallFailure::NotFound => {
            Ok(CardInstallFailure::NotFound)
        }
        golem_api_grpc::proto::golem::worker::CardInstallFailure::RecipientMismatch => {
            Ok(CardInstallFailure::RecipientMismatch)
        }
        golem_api_grpc::proto::golem::worker::CardInstallFailure::NotPermitted => {
            Ok(CardInstallFailure::NotPermitted)
        }
    }
}

pub(super) fn card_install_failure_to_proto(
    value: CardInstallFailure,
) -> golem_api_grpc::proto::golem::worker::CardInstallFailure {
    match value {
        CardInstallFailure::CardRevoked => {
            golem_api_grpc::proto::golem::worker::CardInstallFailure::CardRevoked
        }
        CardInstallFailure::NotFound => {
            golem_api_grpc::proto::golem::worker::CardInstallFailure::NotFound
        }
        CardInstallFailure::RecipientMismatch => {
            golem_api_grpc::proto::golem::worker::CardInstallFailure::RecipientMismatch
        }
        CardInstallFailure::NotPermitted => {
            golem_api_grpc::proto::golem::worker::CardInstallFailure::NotPermitted
        }
    }
}

pub(super) fn raw_card_install_failure_from_proto(
    value: golem_api_grpc::proto::golem::worker::RawCardInstallFailure,
) -> Result<CardInstallFailure, String> {
    match value {
        golem_api_grpc::proto::golem::worker::RawCardInstallFailure::Unspecified => {
            Err("Unspecified raw card install failure".to_string())
        }
        golem_api_grpc::proto::golem::worker::RawCardInstallFailure::CardRevoked => {
            Ok(CardInstallFailure::CardRevoked)
        }
        golem_api_grpc::proto::golem::worker::RawCardInstallFailure::NotFound => {
            Ok(CardInstallFailure::NotFound)
        }
        golem_api_grpc::proto::golem::worker::RawCardInstallFailure::RecipientMismatch => {
            Ok(CardInstallFailure::RecipientMismatch)
        }
        golem_api_grpc::proto::golem::worker::RawCardInstallFailure::NotPermitted => {
            Ok(CardInstallFailure::NotPermitted)
        }
    }
}

pub(super) fn raw_card_install_failure_to_proto(
    value: CardInstallFailure,
) -> golem_api_grpc::proto::golem::worker::RawCardInstallFailure {
    match value {
        CardInstallFailure::CardRevoked => {
            golem_api_grpc::proto::golem::worker::RawCardInstallFailure::CardRevoked
        }
        CardInstallFailure::NotFound => {
            golem_api_grpc::proto::golem::worker::RawCardInstallFailure::NotFound
        }
        CardInstallFailure::RecipientMismatch => {
            golem_api_grpc::proto::golem::worker::RawCardInstallFailure::RecipientMismatch
        }
        CardInstallFailure::NotPermitted => {
            golem_api_grpc::proto::golem::worker::RawCardInstallFailure::NotPermitted
        }
    }
}
