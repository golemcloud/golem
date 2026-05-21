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

use crate::base_model::card::{
    Card, OwnerPathPattern, PathSegmentPattern, PatternGrant, RecipientPathPattern,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum CardAlgebraError {
    InvalidOwnerPath(String),
    InvalidRecipientPath(String),
    DerivationNotSubsumed { grant: PatternGrant },
}

impl PatternGrant {
    pub fn subsumes(&self, other: &Self) -> Result<bool, CardAlgebraError> {
        Ok(self.owner.subsumes(&other.owner)?
            && self.recipient.subsumes(&other.recipient)?
            && self.permission.subsumes(&other.permission))
    }

    pub fn applies_to_recipient(
        &self,
        holder: &RecipientPathPattern,
    ) -> Result<bool, CardAlgebraError> {
        self.recipient.matches_holder(holder)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantSurface {
    pub positive: Vec<PatternGrant>,
    pub negative: Vec<PatternGrant>,
}

impl GrantSurface {
    pub fn allows(&self, request: &PatternGrant) -> Result<bool, CardAlgebraError> {
        let granted = self
            .positive
            .iter()
            .any(|grant| grant.subsumes(request).unwrap_or(false));
        if !granted {
            return Ok(false);
        }

        let denied = self
            .negative
            .iter()
            .any(|grant| grant.subsumes(request).unwrap_or(false));
        Ok(!denied)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveSurface {
    lower: GrantSurface,
    upper: Vec<GrantSurface>,
}

impl EffectiveSurface {
    pub fn from_cards(
        cards: &[Card],
        holder: &RecipientPathPattern,
    ) -> Result<Self, CardAlgebraError> {
        let mut lower_positive = Vec::new();
        let mut lower_negative = Vec::new();
        let mut upper = Vec::new();

        for card in cards {
            lower_positive.extend(filter_by_recipient(&card.lower_positive, holder)?);
            lower_negative.extend(filter_by_recipient(&card.lower_negative, holder)?);

            let upper_positive = filter_by_recipient(&card.upper_positive, holder)?;
            let upper_negative = filter_by_recipient(&card.upper_negative, holder)?;
            if !upper_positive.is_empty() || !upper_negative.is_empty() {
                upper.push(GrantSurface {
                    positive: upper_positive,
                    negative: upper_negative,
                });
            }
        }

        Ok(Self {
            lower: GrantSurface {
                positive: lower_positive,
                negative: lower_negative,
            },
            upper,
        })
    }

    pub fn authorize(&self, request: &PatternGrant) -> Result<bool, CardAlgebraError> {
        if !self.lower.allows(request)? {
            return Ok(false);
        }

        for surface in &self.upper {
            if !surface.allows(request)? {
                return Ok(false);
            }
        }

        Ok(true)
    }

    pub fn validates_derivation(
        parent_cards: &[Card],
        holder: &RecipientPathPattern,
        child_lower: &[PatternGrant],
        child_upper: &[PatternGrant],
    ) -> Result<(), CardAlgebraError> {
        for grant in child_lower {
            if !union_lower_allows(parent_cards, holder, grant)? {
                return Err(CardAlgebraError::DerivationNotSubsumed {
                    grant: grant.clone(),
                });
            }
        }

        for grant in child_upper {
            if !union_upper_allows(parent_cards, holder, grant)? {
                return Err(CardAlgebraError::DerivationNotSubsumed {
                    grant: grant.clone(),
                });
            }
        }

        Ok(())
    }
}

impl OwnerPathPattern {
    pub fn subsumes(&self, other: &Self) -> Result<bool, CardAlgebraError> {
        let left = parse_path(&self.0).map_err(CardAlgebraError::InvalidOwnerPath)?;
        let right = parse_path(&other.0).map_err(CardAlgebraError::InvalidOwnerPath)?;
        Ok(path_subsumes(&left, &right))
    }
}

impl RecipientPathPattern {
    pub fn subsumes(&self, other: &Self) -> Result<bool, CardAlgebraError> {
        Ok(typed_path_subsumes(&self.segments(), &other.segments()))
    }

    pub fn matches_holder(&self, holder: &Self) -> Result<bool, CardAlgebraError> {
        let pattern = self.segments();
        let holder = holder.segments();
        Ok(
            pattern.len() <= holder.len()
                && typed_path_subsumes(&pattern, &holder[..pattern.len()]),
        )
    }
}

fn filter_by_recipient(
    grants: &[PatternGrant],
    holder: &RecipientPathPattern,
) -> Result<Vec<PatternGrant>, CardAlgebraError> {
    grants
        .iter()
        .filter_map(|grant| match grant.applies_to_recipient(holder) {
            Ok(true) => Some(Ok(grant.clone())),
            Ok(false) => None,
            Err(err) => Some(Err(err)),
        })
        .collect()
}

fn union_lower_allows(
    cards: &[Card],
    holder: &RecipientPathPattern,
    grant: &PatternGrant,
) -> Result<bool, CardAlgebraError> {
    for card in cards {
        let surface = GrantSurface {
            positive: filter_by_recipient(&card.lower_positive, holder)?,
            negative: filter_by_recipient(&card.lower_negative, holder)?,
        };
        if surface.allows(grant)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn union_upper_allows(
    cards: &[Card],
    holder: &RecipientPathPattern,
    grant: &PatternGrant,
) -> Result<bool, CardAlgebraError> {
    for card in cards {
        let positive = filter_by_recipient(&card.upper_positive, holder)?;
        let negative = filter_by_recipient(&card.upper_negative, holder)?;
        if positive.is_empty() && negative.is_empty() {
            return Ok(true);
        }
        if (GrantSurface { positive, negative }).allows(grant)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn parse_path(path: &str) -> Result<Vec<&str>, String> {
    if path.is_empty() {
        Ok(Vec::new())
    } else if path.split('/').any(str::is_empty) {
        Err(path.to_string())
    } else {
        Ok(path.split('/').collect())
    }
}

fn path_subsumes(left: &[&str], right: &[&str]) -> bool {
    let max_len = left.len().max(right.len());
    for idx in 0..max_len {
        let left = left.get(idx).copied().unwrap_or("*");
        let right = right.get(idx).copied().unwrap_or("*");
        if !segment_subsumes(left, right) {
            return false;
        }
    }
    true
}

fn segment_subsumes(left: &str, right: &str) -> bool {
    left == "*" || left == right
}

fn typed_path_subsumes(left: &[PathSegmentPattern], right: &[PathSegmentPattern]) -> bool {
    let max_len = left.len().max(right.len());
    for idx in 0..max_len {
        let left = left.get(idx).cloned().unwrap_or(PathSegmentPattern::Any);
        let right = right.get(idx).cloned().unwrap_or(PathSegmentPattern::Any);
        if !left.subsumes(&right) {
            return false;
        }
    }
    true
}
