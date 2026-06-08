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
use crate::base_model::card::{Card, PermissionPattern, PermissionTarget};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum CardAlgebraError {
    InvalidOwnerPath(String),
    InvalidRecipientPath(String),
    DerivationNotSubsumed { grant: Box<PermissionPattern> },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GrantSurface {
    pub positive: Vec<PermissionPattern>,
    pub negative: Vec<PermissionPattern>,
}

impl GrantSurface {
    pub fn allows(&self, request: &PermissionTarget) -> Result<bool, CardAlgebraError> {
        let granted = self
            .positive
            .iter()
            .any(|grant| grant.subsumes_target(request));
        if !granted {
            return Ok(false);
        }

        let denied = self
            .negative
            .iter()
            .any(|grant| grant.subsumes_target(request));
        Ok(!denied)
    }

    pub fn allows_ceiling(&self, request: &PermissionTarget) -> Result<bool, CardAlgebraError> {
        let granted = self.positive.is_empty()
            || self
                .positive
                .iter()
                .any(|grant| grant.subsumes_target(request));
        if !granted {
            return Ok(false);
        }

        let denied = self
            .negative
            .iter()
            .any(|grant| grant.subsumes_target(request));
        Ok(!denied)
    }

    pub fn allows_grant(&self, request: &PermissionPattern) -> Result<bool, CardAlgebraError> {
        let granted = self.positive.iter().any(|grant| grant.subsumes(request));
        if !granted {
            return Ok(false);
        }

        let denied = self.negative.iter().any(|grant| grant.subsumes(request));
        Ok(!denied)
    }

    pub fn allows_grant_ceiling(
        &self,
        request: &PermissionPattern,
    ) -> Result<bool, CardAlgebraError> {
        let granted =
            self.positive.is_empty() || self.positive.iter().any(|grant| grant.subsumes(request));
        if !granted {
            return Ok(false);
        }

        let denied = self.negative.iter().any(|grant| grant.subsumes(request));
        Ok(!denied)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EffectiveSurface {
    lower: Vec<GrantSurface>,
    upper: Vec<GrantSurface>,
}

impl EffectiveSurface {
    pub fn from_cards(cards: &[Card], holder: &RecipientPattern) -> Result<Self, CardAlgebraError> {
        let mut lower = Vec::new();
        let mut upper = Vec::new();

        for card in cards {
            let lower_positive = filter_by_recipient(&card.lower_positive, holder)?;
            let lower_negative = filter_by_recipient(&card.lower_negative, holder)?;
            if !lower_positive.is_empty() {
                lower.push(GrantSurface {
                    positive: lower_positive,
                    negative: lower_negative,
                });
            }

            let upper_positive = filter_by_recipient(&card.upper_positive, holder)?;
            let upper_negative = filter_by_recipient(&card.upper_negative, holder)?;
            if !upper_positive.is_empty() || !upper_negative.is_empty() {
                upper.push(GrantSurface {
                    positive: upper_positive,
                    negative: upper_negative,
                });
            }
        }

        Ok(Self { lower, upper })
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

        Ok(Self { lower, upper })
    }

    pub fn authorize(&self, request: &PermissionTarget) -> Result<bool, CardAlgebraError> {
        if !self.allows_lower(request)? {
            return Ok(false);
        }

        self.allows_upper(request)
    }

    pub fn validates_derivation(
        &self,
        child_lower_positive: &[PermissionPattern],
        child_upper_positive: &[PermissionPattern],
    ) -> Result<(), CardAlgebraError> {
        for grant in child_lower_positive {
            if !self.allows_lower_grant(grant)? {
                return Err(CardAlgebraError::DerivationNotSubsumed {
                    grant: Box::new(grant.clone()),
                });
            }
        }

        for grant in child_upper_positive {
            if !self.allows_upper_grant(grant)? {
                return Err(CardAlgebraError::DerivationNotSubsumed {
                    grant: Box::new(grant.clone()),
                });
            }
        }

        Ok(())
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

    fn allows_lower_grant(&self, request: &PermissionPattern) -> Result<bool, CardAlgebraError> {
        for surface in &self.lower {
            if surface.allows_grant(request)? {
                return Ok(true);
            }
        }

        Ok(false)
    }

    fn allows_upper_grant(&self, request: &PermissionPattern) -> Result<bool, CardAlgebraError> {
        for surface in &self.upper {
            if !surface.allows_grant_ceiling(request)? {
                return Ok(false);
            }
        }

        Ok(true)
    }
}

fn filter_by_recipient(
    grants: &[PermissionPattern],
    holder: &RecipientPattern,
) -> Result<Vec<PermissionPattern>, CardAlgebraError> {
    Ok(grants
        .iter()
        .filter(|grant| grant.recipient().subsumes(holder))
        .cloned()
        .collect())
}
