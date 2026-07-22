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

use crate::base_model::card::recipient::RecipientPattern;
use crate::base_model::card::{Card, CardId, PermissionPattern, PermissionTarget};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum CardAlgebraError {
    InvalidOwnerPath(String),
    InvalidRecipientPath(String),
    LowerBoundTooBroad {
        grant: Box<PermissionPattern>,
    },
    UpperBoundTooBroad {
        grant: Option<Box<PermissionPattern>>,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct GrantSurface {
    pub positive: Vec<PermissionTarget>,
    pub negative: Vec<PermissionTarget>,
}

impl GrantSurface {
    pub fn allows(&self, request: &PermissionTarget) -> Result<bool, CardAlgebraError> {
        let granted = self.positive.iter().any(|grant| grant.subsumes(request));
        if !granted {
            return Ok(false);
        }

        let denied = self.negative.iter().any(|grant| grant.subsumes(request));
        Ok(!denied)
    }

    pub fn allows_ceiling(&self, request: &PermissionTarget) -> Result<bool, CardAlgebraError> {
        let granted =
            self.positive.is_empty() || self.positive.iter().any(|grant| grant.subsumes(request));
        if !granted {
            return Ok(false);
        }

        let denied = self.negative.iter().any(|grant| grant.subsumes(request));
        Ok(!denied)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct EffectiveSurface {
    pub source_card_ids: Vec<CardId>,
    pub lower: Vec<GrantSurface>,
    pub upper: Vec<GrantSurface>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct DelegationCard {
    pub source_card_id: Option<CardId>,
    pub lower_positive: Vec<PermissionPattern>,
    pub lower_negative: Vec<PermissionPattern>,
    pub upper_positive: Vec<PermissionPattern>,
    pub upper_negative: Vec<PermissionPattern>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct DelegationSurface {
    pub cards: Vec<DelegationCard>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalletDerivationParent {
    Single(CardId),
    MultipleRequired,
    NotPermitted,
}

impl EffectiveSurface {
    pub fn from_cards(cards: &[Card], holder: &RecipientPattern) -> Result<Self, CardAlgebraError> {
        let mut lower = Vec::new();
        let mut upper = Vec::new();
        let source_card_ids = cards.iter().map(|card| card.card_id).collect();

        for card in cards {
            let lower_positive = filter_by_recipient(&card.lower_positive, holder)?;
            let lower_negative = filter_by_recipient(&card.lower_negative, holder)?;
            lower.push(GrantSurface {
                positive: lower_positive,
                negative: lower_negative,
            });

            let upper_positive = filter_by_recipient(&card.upper_positive, holder)?;
            let upper_negative = filter_by_recipient(&card.upper_negative, holder)?;
            upper.push(GrantSurface {
                positive: upper_positive,
                negative: upper_negative,
            });
        }

        Ok(Self {
            source_card_ids,
            lower,
            upper,
        })
    }

    pub fn from_grants(
        lower_positive: &[PermissionPattern],
        lower_negative: &[PermissionPattern],
        upper_positive: &[PermissionPattern],
        upper_negative: &[PermissionPattern],
        holder: &RecipientPattern,
    ) -> Result<Self, CardAlgebraError> {
        let mut lower = Vec::new();
        let lower_positive = filter_by_recipient(lower_positive, holder)?;
        let lower_negative = filter_by_recipient(lower_negative, holder)?;
        if !lower_positive.is_empty() {
            lower.push(GrantSurface {
                positive: lower_positive,
                negative: lower_negative,
            });
        }

        let mut upper = Vec::new();
        let upper_positive = filter_by_recipient(upper_positive, holder)?;
        let upper_negative = filter_by_recipient(upper_negative, holder)?;
        if !upper_positive.is_empty() || !upper_negative.is_empty() {
            upper.push(GrantSurface {
                positive: upper_positive,
                negative: upper_negative,
            });
        }

        Ok(Self {
            source_card_ids: Vec::new(),
            lower,
            upper,
        })
    }

    pub fn authorize(&self, request: &PermissionTarget) -> Result<bool, CardAlgebraError> {
        if !self.allows_lower(request)? {
            return Ok(false);
        }

        self.allows_upper(request)
    }

    fn allows_lower(&self, request: &PermissionTarget) -> Result<bool, CardAlgebraError> {
        for surface in &self.lower {
            if surface.allows(request)? {
                return Ok(true);
            }
        }

        Ok(false)
    }

    fn allows_upper(&self, request: &PermissionTarget) -> Result<bool, CardAlgebraError> {
        for surface in &self.upper {
            if !surface.allows_ceiling(request)? {
                return Ok(false);
            }
        }

        Ok(true)
    }
}

impl DelegationSurface {
    pub fn from_cards(cards: &[Card]) -> Self {
        Self {
            cards: cards
                .iter()
                .map(|card| DelegationCard {
                    source_card_id: Some(card.card_id),
                    lower_positive: card.lower_positive.clone(),
                    lower_negative: card.lower_negative.clone(),
                    upper_positive: card.upper_positive.clone(),
                    upper_negative: card.upper_negative.clone(),
                })
                .collect(),
        }
    }

    /// Validates resulting child bounds. Child negatives must include inherited denials plus additions.
    pub fn validate_attenuation(
        &self,
        child_lower_positive: &[PermissionPattern],
        child_lower_negative: &[PermissionPattern],
        child_upper_positive: &[PermissionPattern],
        child_upper_negative: &[PermissionPattern],
    ) -> Result<Vec<CardId>, CardAlgebraError> {
        validate_attenuation_for_cards(
            &self.cards,
            child_lower_positive,
            child_lower_negative,
            child_upper_positive,
            child_upper_negative,
        )
    }

    pub fn select_wallet_derivation_parent(
        &self,
        child_lower_positive: &[PermissionPattern],
        child_lower_negative: &[PermissionPattern],
        child_upper_positive: &[PermissionPattern],
        child_upper_negative: &[PermissionPattern],
    ) -> Result<WalletDerivationParent, CardAlgebraError> {
        if self.cards.is_empty() {
            return Ok(WalletDerivationParent::NotPermitted);
        }

        let selected = self
            .cards
            .iter()
            .filter_map(|card| {
                validate_attenuation_for_cards(
                    std::slice::from_ref(card),
                    child_lower_positive,
                    child_lower_negative,
                    child_upper_positive,
                    child_upper_negative,
                )
                .ok()
                .and(card.source_card_id)
            })
            .max();

        if let Some(card_id) = selected {
            return Ok(WalletDerivationParent::Single(card_id));
        }

        self.validate_attenuation(
            child_lower_positive,
            child_lower_negative,
            child_upper_positive,
            child_upper_negative,
        )?;
        Ok(WalletDerivationParent::MultipleRequired)
    }
}

fn validate_attenuation_for_cards(
    cards: &[DelegationCard],
    child_lower_positive: &[PermissionPattern],
    child_lower_negative: &[PermissionPattern],
    child_upper_positive: &[PermissionPattern],
    child_upper_negative: &[PermissionPattern],
) -> Result<Vec<CardId>, CardAlgebraError> {
    let mut witness = Vec::new();

    for grant in child_lower_positive {
        let mut parent = None;
        for card in cards {
            if positive_subsumes(&card.lower_positive, grant)
                && negatives_are_preserved(&card.lower_negative, child_lower_negative)
            {
                parent = Some(card);
                break;
            }
        }
        let Some(parent) = parent else {
            return Err(CardAlgebraError::LowerBoundTooBroad {
                grant: Box::new(grant.clone()),
            });
        };
        push_source_if_available(&mut witness, parent.source_card_id);
    }

    for card in cards {
        if !negatives_are_preserved(&card.upper_negative, child_upper_negative) {
            return Err(CardAlgebraError::UpperBoundTooBroad {
                grant: child_upper_positive
                    .first()
                    .or(child_lower_positive.first())
                    .cloned()
                    .map(Box::new),
            });
        }

        // Empty upper positives denote an implicit top ceiling. Recipient coverage
        // keeps a finite parent ceiling from becoming top for excluded holders.
        if (!card.upper_positive.is_empty() && child_upper_positive.is_empty())
            || !parent_upper_recipients_are_covered(&card.upper_positive, child_upper_positive)
        {
            return Err(CardAlgebraError::UpperBoundTooBroad { grant: None });
        }

        for grant in child_upper_positive {
            if !ceiling_positive_subsumes(&card.upper_positive, grant) {
                return Err(CardAlgebraError::UpperBoundTooBroad {
                    grant: Some(Box::new(grant.clone())),
                });
            }
        }

        if !upper_is_empty(card) {
            push_source_if_available(&mut witness, card.source_card_id);
        }
    }

    Ok(witness)
}

fn positive_subsumes(positive: &[PermissionPattern], request: &PermissionPattern) -> bool {
    positive.iter().any(|grant| grant.subsumes(request))
}

fn ceiling_positive_subsumes(positive: &[PermissionPattern], request: &PermissionPattern) -> bool {
    let request_recipient = request.recipient();
    let request_target = request.to_target();

    positive
        .iter()
        .filter(|grant| recipients_overlap(grant.recipient(), request_recipient))
        .all(|overlapping_grant| {
            let overlap_recipient = if request_recipient.subsumes(overlapping_grant.recipient()) {
                overlapping_grant.recipient()
            } else {
                request_recipient
            };
            positive.iter().any(|grant| {
                grant.recipient().subsumes(overlap_recipient)
                    && grant.subsumes_target(&request_target)
            })
        })
}

fn parent_upper_recipients_are_covered(
    parent: &[PermissionPattern],
    child: &[PermissionPattern],
) -> bool {
    parent.iter().all(|parent_grant| {
        child
            .iter()
            .any(|child_grant| child_grant.recipient().subsumes(parent_grant.recipient()))
    })
}

fn recipients_overlap(left: &RecipientPattern, right: &RecipientPattern) -> bool {
    left.subsumes(right) || right.subsumes(left)
}

fn negatives_are_preserved(
    parent_negative: &[PermissionPattern],
    child_negative: &[PermissionPattern],
) -> bool {
    parent_negative
        .iter()
        .all(|parent| child_negative.iter().any(|child| child.subsumes(parent)))
}

fn upper_is_empty(card: &DelegationCard) -> bool {
    card.upper_positive.is_empty() && card.upper_negative.is_empty()
}

fn push_source_if_available(values: &mut Vec<CardId>, source_card_id: Option<CardId>) {
    if let Some(card_id) = source_card_id {
        push_unique(values, card_id);
    }
}

fn push_unique(values: &mut Vec<CardId>, value: CardId) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn filter_by_recipient(
    grants: &[PermissionPattern],
    holder: &RecipientPattern,
) -> Result<Vec<PermissionTarget>, CardAlgebraError> {
    Ok(grants
        .iter()
        .filter(|grant| grant.recipient().subsumes(holder))
        .map(PermissionPattern::to_target)
        .collect())
}
