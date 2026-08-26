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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

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
