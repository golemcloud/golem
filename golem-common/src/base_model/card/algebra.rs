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

use crate::base_model::card::owner::{AgentOwnerLeafPattern, AgentOwnerPattern, OwnerPattern};
use crate::base_model::card::recipient::RecipientPattern;
use crate::base_model::card::{
    Card, CardId, NetworkClass, PermissionClass, PermissionPattern, PermissionTarget,
    ToolResourcePattern,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum CardAlgebraError {
    InvalidOwnerPath(String),
    InvalidRecipientPath(String),
    ToolPattern(String),
    /// No parent card grants the child's lower-bound grant.
    LowerBoundTooBroad {
        grant: Box<PermissionPattern>,
    },
    /// A parent card grants the child's lower-bound grant, but the child does not keep one of
    /// that parent's lower-bound denials.
    LowerDenialNotKept {
        grant: Box<PermissionPattern>,
        denial: Box<PermissionPattern>,
    },
    /// The child does not keep one of the parent's upper-bound denials.
    UpperDenialNotKept {
        denial: Box<PermissionPattern>,
    },
    /// The parent has an upper-bound ceiling, and the child's upper-bound grants do not cover
    /// the recipient of this ceiling grant. An empty list of upper-bound grants is no ceiling.
    UpperCeilingNotKept {
        ceiling: Box<PermissionPattern>,
    },
    /// A child upper-bound grant is broader than the parent's ceiling.
    UpperBoundTooBroad {
        grant: Box<PermissionPattern>,
    },
    /// A wallet derivation where no single wallet card and not the whole wallet permits the
    /// child. `combined` is the failure of the whole wallet; `cards` holds each card's own
    /// failure, ordered by card ID.
    NoWalletCardFits {
        combined: Box<CardAlgebraError>,
        cards: Vec<WalletCardFailure>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct WalletCardFailure {
    pub card_id: CardId,
    pub error: CardAlgebraError,
}

/// The number of wallet cards whose failure a [`CardAlgebraError::NoWalletCardFits`] message
/// lists before it counts the rest.
const LISTED_WALLET_CARD_FAILURES: usize = 3;

impl CardAlgebraError {
    /// The message for a failed wallet derivation. A wallet derivation takes complete bounds and
    /// inherits nothing from the wallet card, so when restating a denial or a ceiling fixes the
    /// request, the message also says what to restate.
    pub fn wallet_derivation_message(&self) -> String {
        match self.wallet_derivation_hint() {
            Some(hint) => format!("{self}; {hint}"),
            None => self.to_string(),
        }
    }

    fn wallet_derivation_hint(&self) -> Option<&'static str> {
        match self {
            Self::LowerDenialNotKept { .. } => Some(
                "derive-from-wallet takes complete bounds, so restate this denial in the lower-negative grants of the request",
            ),
            Self::UpperDenialNotKept { .. } => Some(
                "derive-from-wallet takes complete bounds, so restate this denial in the upper-negative grants of the request",
            ),
            Self::UpperCeilingNotKept { .. } => Some(
                "derive-from-wallet takes complete bounds, so restate this ceiling or a narrower one in the upper-positive grants of the request",
            ),
            Self::NoWalletCardFits { combined, .. } => combined.wallet_derivation_hint(),
            _ => None,
        }
    }
}

impl std::fmt::Display for CardAlgebraError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn grant(grant: &PermissionPattern) -> String {
            grant.render().unwrap_or_else(|_| format!("{grant:?}"))
        }

        match self {
            Self::InvalidOwnerPath(owner) => write!(f, "invalid owner path `{owner}`"),
            Self::InvalidRecipientPath(recipient) => {
                write!(f, "invalid recipient path `{recipient}`")
            }
            Self::ToolPattern(message) => write!(f, "invalid tool pattern: {message}"),
            Self::LowerBoundTooBroad { grant: child } => write!(
                f,
                "no parent card grants the lower-bound grant `{}`",
                grant(child)
            ),
            Self::LowerDenialNotKept {
                grant: child,
                denial,
            } => write!(
                f,
                "the child card must keep the parent's lower-bound denial `{}` for the lower-bound grant `{}`",
                grant(denial),
                grant(child)
            ),
            Self::UpperDenialNotKept { denial } => write!(
                f,
                "the child card must keep the parent's upper-bound denial `{}`",
                grant(denial)
            ),
            Self::UpperCeilingNotKept { ceiling } => write!(
                f,
                "the parent card has the upper-bound ceiling `{}`, which the child card does not keep; an empty upper-positive list means no ceiling",
                grant(ceiling)
            ),
            Self::UpperBoundTooBroad { grant: child } => write!(
                f,
                "the upper-bound grant `{}` is broader than the parent card's ceiling",
                grant(child)
            ),
            Self::NoWalletCardFits { combined, cards } => {
                write!(
                    f,
                    "no wallet card permits the requested derivation: across the whole wallet, {combined}"
                )?;
                for failure in cards.iter().take(LISTED_WALLET_CARD_FAILURES) {
                    write!(f, "; card {}: {}", failure.card_id, failure.error)?;
                }
                if cards.len() > LISTED_WALLET_CARD_FAILURES {
                    write!(f, "; +{} more", cards.len() - LISTED_WALLET_CARD_FAILURES)?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct GrantSurface {
    pub positive: Vec<PermissionTarget>,
    pub negative: Vec<PermissionTarget>,
}

impl GrantSurface {
    pub fn allows(&self, request: &PermissionTarget) -> Result<bool, CardAlgebraError> {
        let granted = matches_any(&self.positive, request, false)?;
        if !granted {
            return Ok(false);
        }

        let denied = matches_any(&self.negative, request, true)?;
        Ok(!denied)
    }

    pub fn allows_ceiling(&self, request: &PermissionTarget) -> Result<bool, CardAlgebraError> {
        let granted = self.positive.is_empty() || matches_any(&self.positive, request, false)?;
        if !granted {
            return Ok(false);
        }

        let denied = matches_any(&self.negative, request, true)?;
        Ok(!denied)
    }
}

fn matches_any(
    grants: &[PermissionTarget],
    request: &PermissionTarget,
    denial: bool,
) -> Result<bool, CardAlgebraError> {
    for grant in grants {
        let matches = match (grant, request) {
            (PermissionTarget::Tool(grant), PermissionTarget::Tool(request))
                if matches!(request.resource, ToolResourcePattern::Request(_)) =>
            {
                if !grant.owner.subsumes(&request.owner)
                    || (grant.verb.is_some() && grant.verb != request.verb)
                {
                    false
                } else {
                    match (&grant.resource, &request.resource) {
                        (ToolResourcePattern::AnyInvocation, _) => true,
                        (
                            ToolResourcePattern::Invocation(pattern),
                            ToolResourcePattern::Request(invocation),
                        ) => if denial {
                            pattern.possibly_matches(invocation)
                        } else {
                            pattern.definitely_matches(invocation)
                        }
                        .map_err(|e| CardAlgebraError::ToolPattern(e.to_string()))?,
                        _ => false,
                    }
                }
            }
            (PermissionTarget::Network(grant), PermissionTarget::Network(request)) if denial => {
                grant.owner.subsumes(&request.owner)
                    && NetworkClass::verb_subsumes(grant.verb, request.verb)
                    && grant.resource.denies(&request.resource)
            }
            _ if denial => {
                grant.subsumes(request)
                    || typed_like_denial(grant, request).is_some_and(|typed| grant.subsumes(&typed))
            }
            _ => grant.subsumes(request),
        };
        if matches {
            return Ok(true);
        }
    }
    Ok(false)
}

/// A concrete agent in a request may lack its trusted type name, for example when the target
/// was parsed from text. A `Type(*)` denial must still apply to it unless its reserved tag
/// shows that it is no agent of any type, so a denial assumes the untyped agent has the
/// wildcard's type. Grants never do: an untyped agent is not covered by a type wildcard grant.
fn typed_like_denial(
    denial: &PermissionTarget,
    request: &PermissionTarget,
) -> Option<PermissionTarget> {
    let AgentOwnerPattern::Agent {
        agent: AgentOwnerLeafPattern::AgentTypeWildcard(agent_type),
        ..
    } = agent_owner(denial)?
    else {
        return None;
    };
    let mut typed = request.clone();
    match agent_owner_mut(&mut typed)? {
        AgentOwnerPattern::Agent {
            agent: AgentOwnerLeafPattern::Agent(leaf),
            ..
        } if leaf.agent_type.is_none() && leaf.agent_id.type_id().is_some() => {
            leaf.agent_type = Some(agent_type.clone());
            Some(typed)
        }
        _ => None,
    }
}

fn agent_owner(target: &PermissionTarget) -> Option<&AgentOwnerPattern> {
    match target {
        PermissionTarget::Filesystem(target) => Some(&target.owner),
        PermissionTarget::Env(target) => Some(&target.owner),
        PermissionTarget::Oplog(target) => Some(&target.owner),
        PermissionTarget::Config(target) => Some(&target.owner),
        PermissionTarget::Agent(target) => Some(&target.owner),
        _ => None,
    }
}

fn agent_owner_mut(target: &mut PermissionTarget) -> Option<&mut AgentOwnerPattern> {
    match target {
        PermissionTarget::Filesystem(target) => Some(&mut target.owner),
        PermissionTarget::Env(target) => Some(&mut target.owner),
        PermissionTarget::Oplog(target) => Some(&mut target.owner),
        PermissionTarget::Config(target) => Some(&mut target.owner),
        PermissionTarget::Agent(target) => Some(&mut target.owner),
        _ => None,
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

        let mut selected = None;
        let mut failures = Vec::new();
        for card in &self.cards {
            let result = validate_attenuation_for_cards(
                std::slice::from_ref(card),
                child_lower_positive,
                child_lower_negative,
                child_upper_positive,
                child_upper_negative,
            );
            match (result, card.source_card_id) {
                (Ok(_), Some(card_id)) => selected = selected.max(Some(card_id)),
                (Err(error), Some(card_id)) => failures.push(WalletCardFailure { card_id, error }),
                (_, None) => {}
            }
        }

        if let Some(card_id) = selected {
            return Ok(WalletDerivationParent::Single(card_id));
        }

        // With one card, the whole-wallet check is that card's own check, so its failure is the
        // exact reason.
        self.validate_attenuation(
            child_lower_positive,
            child_lower_negative,
            child_upper_positive,
            child_upper_negative,
        )
        .map_err(|combined| {
            if failures.len() < 2 {
                return combined;
            }
            failures.sort_by_key(|failure| failure.card_id);
            CardAlgebraError::NoWalletCardFits {
                combined: Box::new(combined),
                cards: failures,
            }
        })?;
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
            let dropped_denial = cards
                .iter()
                .filter(|card| positive_subsumes(&card.lower_positive, grant))
                .find_map(|card| {
                    first_dropped_negative(&card.lower_negative, child_lower_negative)
                });
            return Err(match dropped_denial {
                Some(denial) => CardAlgebraError::LowerDenialNotKept {
                    grant: Box::new(grant.clone()),
                    denial: Box::new(denial.clone()),
                },
                None => CardAlgebraError::LowerBoundTooBroad {
                    grant: Box::new(grant.clone()),
                },
            });
        };
        push_source_if_available(&mut witness, parent.source_card_id);
    }

    for card in cards {
        if let Some(denial) = first_dropped_negative(&card.upper_negative, child_upper_negative) {
            return Err(CardAlgebraError::UpperDenialNotKept {
                denial: Box::new(denial.clone()),
            });
        }

        // Empty upper positives denote an implicit top ceiling. Recipient coverage
        // keeps a finite parent ceiling from becoming top for excluded holders.
        if let Some(ceiling) =
            first_uncovered_parent_ceiling(&card.upper_positive, child_upper_positive)
        {
            return Err(CardAlgebraError::UpperCeilingNotKept {
                ceiling: Box::new(ceiling.clone()),
            });
        }

        for grant in child_upper_positive {
            if !ceiling_positive_subsumes(&card.upper_positive, grant) {
                return Err(CardAlgebraError::UpperBoundTooBroad {
                    grant: Box::new(grant.clone()),
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

/// The first parent ceiling grant whose recipient no child upper-bound grant covers. With an
/// empty child list, this is the first parent ceiling grant.
fn first_uncovered_parent_ceiling<'a>(
    parent: &'a [PermissionPattern],
    child: &[PermissionPattern],
) -> Option<&'a PermissionPattern> {
    parent.iter().find(|parent_grant| {
        !child
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
    first_dropped_negative(parent_negative, child_negative).is_none()
}

fn first_dropped_negative<'a>(
    parent_negative: &'a [PermissionPattern],
    child_negative: &[PermissionPattern],
) -> Option<&'a PermissionPattern> {
    parent_negative
        .iter()
        .find(|parent| !child_negative.iter().any(|child| child.subsumes(parent)))
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
