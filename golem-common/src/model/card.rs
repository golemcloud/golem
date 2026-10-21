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

use std::sync::Arc;

use desert_rust::{
    BinaryCodec, BinaryDeserializer, BinaryOutput, BinarySerializer, DeserializationContext,
    SerializationContext,
};
use im::OrdMap;
use uuid::Uuid;

pub use crate::base_model::card::*;

const SCOPE_CARD_ENCODING_VERSION: u32 = 1;

impl TryFrom<&ScopeCard> for golem_api_grpc::proto::golem::worker::EncodedScopeCard {
    type Error = String;

    fn try_from(value: &ScopeCard) -> Result<Self, Self::Error> {
        Ok(Self {
            encoding_version: SCOPE_CARD_ENCODING_VERSION,
            scope_card_id: Some(value.scope_card_id.0.into()),
            root_card_ids: value
                .root_card_ids
                .iter()
                .map(|card_id| card_id.0.into())
                .collect(),
            lower_positive: desert_rust::serialize_to_byte_vec(&value.lower_positive)
                .map_err(|error| format!("failed to encode lower-positive grants: {error}"))?,
            lower_negative: desert_rust::serialize_to_byte_vec(&value.lower_negative)
                .map_err(|error| format!("failed to encode lower-negative grants: {error}"))?,
            upper_positive: desert_rust::serialize_to_byte_vec(&value.upper_positive)
                .map_err(|error| format!("failed to encode upper-positive grants: {error}"))?,
            upper_negative: desert_rust::serialize_to_byte_vec(&value.upper_negative)
                .map_err(|error| format!("failed to encode upper-negative grants: {error}"))?,
            expires_at: value.expires_at.map(|expires_at| prost_types::Timestamp {
                seconds: expires_at.timestamp(),
                nanos: expires_at.timestamp_subsec_nanos() as i32,
            }),
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::EncodedScopeCard> for ScopeCard {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::EncodedScopeCard,
    ) -> Result<Self, Self::Error> {
        if value.encoding_version != SCOPE_CARD_ENCODING_VERSION {
            return Err(format!(
                "unsupported scope-card encoding version {}",
                value.encoding_version
            ));
        }

        Ok(Self {
            scope_card_id: CardId(value.scope_card_id.ok_or("missing scope-card ID")?.into()),
            root_card_ids: value
                .root_card_ids
                .into_iter()
                .map(|card_id| CardId(card_id.into()))
                .collect(),
            lower_positive: desert_rust::deserialize(&value.lower_positive)
                .map_err(|error| format!("failed to decode lower-positive grants: {error}"))?,
            lower_negative: desert_rust::deserialize(&value.lower_negative)
                .map_err(|error| format!("failed to decode lower-negative grants: {error}"))?,
            upper_positive: desert_rust::deserialize(&value.upper_positive)
                .map_err(|error| format!("failed to decode upper-positive grants: {error}"))?,
            upper_negative: desert_rust::deserialize(&value.upper_negative)
                .map_err(|error| format!("failed to decode upper-negative grants: {error}"))?,
            expires_at: value
                .expires_at
                .map(|expires_at| {
                    u32::try_from(expires_at.nanos)
                        .ok()
                        .and_then(|nanos| {
                            chrono::DateTime::from_timestamp(expires_at.seconds, nanos)
                        })
                        .ok_or("invalid scope-card expiry")
                })
                .transpose()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;

    fn scope_card() -> ScopeCard {
        let grant = parse_permission(
            "agent(acme/shop/prod/cart-svc/ShoppingCart(*)) @ acme/shop/prod/cart-svc/ShoppingCart : invoke : add-item",
        )
        .unwrap();
        ScopeCard {
            scope_card_id: CardId(uuid::Uuid::from_u128(1)),
            root_card_ids: vec![
                CardId(uuid::Uuid::from_u128(2)),
                CardId(uuid::Uuid::from_u128(3)),
            ],
            lower_positive: vec![grant.clone()],
            lower_negative: vec![grant.clone()],
            upper_positive: vec![grant.clone()],
            upper_negative: vec![grant],
            expires_at: chrono::DateTime::from_timestamp(1_800_000_000, 123_456_789),
        }
    }

    #[test]
    fn scope_card_protobuf_round_trip_preserves_full_payload() {
        let scope_card = scope_card();
        let encoded =
            golem_api_grpc::proto::golem::worker::EncodedScopeCard::try_from(&scope_card).unwrap();
        assert_eq!(
            encoded,
            golem_api_grpc::proto::golem::worker::EncodedScopeCard::try_from(&scope_card).unwrap()
        );

        assert_eq!(ScopeCard::try_from(encoded).unwrap(), scope_card);
    }

    #[test]
    fn scope_card_protobuf_rejects_unsupported_versions() {
        let mut encoded =
            golem_api_grpc::proto::golem::worker::EncodedScopeCard::try_from(&scope_card())
                .unwrap();
        encoded.encoding_version += 1;

        assert!(ScopeCard::try_from(encoded).is_err());
    }
}
/// The durable target-side identity associated with a permission-card transfer ID.
///
/// This is stored behind an `Arc` in [`ReceivedCardTransferIndex`]. Boxing the common `Received`
/// payload separately would add an allocation and pointer indirection to every indexed transfer.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
pub enum ReceivedCardTransferState {
    Received {
        source_card_id: CardId,
        card: StoredCard,
    },
    Conflict,
}

/// An oplog-derived, sticky index of target-side permission-card receipts.
///
/// Receipts remain indexed even when their oplog entries are in skipped or deleted regions, so a
/// retry cannot redeliver a transfer that the target has already observed. The persistent map
/// keeps status clones and delta comparisons proportional to the changed paths rather than the
/// total number of transfers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReceivedCardTransferIndex(OrdMap<Uuid, Arc<ReceivedCardTransferState>>);

impl ReceivedCardTransferIndex {
    pub fn get(&self, transfer_id: &Uuid) -> Option<&ReceivedCardTransferState> {
        self.0.get(transfer_id).map(Arc::as_ref)
    }

    pub fn insert(&mut self, transfer_id: Uuid, state: ReceivedCardTransferState) {
        self.0.insert(transfer_id, Arc::new(state));
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Uuid, &ReceivedCardTransferState)> {
        self.0
            .iter()
            .map(|(transfer_id, state)| (transfer_id, state.as_ref()))
    }

    pub fn changes_from<'a>(
        &'a self,
        previous: &'a Self,
    ) -> impl Iterator<Item = (Uuid, Option<&'a ReceivedCardTransferState>)> + 'a {
        use im::ordmap::DiffItem;

        previous.0.diff(&self.0).map(|change| match change {
            DiffItem::Add(transfer_id, state) => (*transfer_id, Some(state.as_ref())),
            DiffItem::Update {
                new: (transfer_id, state),
                ..
            } => (*transfer_id, Some(state.as_ref())),
            DiffItem::Remove(transfer_id, _) => (*transfer_id, None),
        })
    }
}

impl BinarySerializer for ReceivedCardTransferIndex {
    fn serialize<Output: BinaryOutput>(
        &self,
        context: &mut SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        desert_rust::serialize_iterator(&mut self.0.iter(), context)
    }
}

impl BinaryDeserializer for ReceivedCardTransferIndex {
    fn deserialize(context: &mut DeserializationContext<'_>) -> desert_rust::Result<Self> {
        let entries =
            desert_rust::deserialize_iterator::<(Uuid, Arc<ReceivedCardTransferState>)>(context)
                .0
                .collect::<desert_rust::Result<OrdMap<_, _>>>()?;
        Ok(Self(entries))
    }
}
