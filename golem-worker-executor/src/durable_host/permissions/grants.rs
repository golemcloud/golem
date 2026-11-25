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
    CardAlgebraError, CardParseError, CardVerb, DurableWorkerCtx, PermissionPattern,
    PolymorphicPermissionPattern, RenderedPermissionFields, ResolvedPermissionCard, StoredCard,
    WorkerCtx, parse_permission_fields, parse_polymorphic_permission_fields, permissions_types,
};

impl From<jiff::Timestamp> for permissions_types::Timestamp {
    fn from(timestamp: jiff::Timestamp) -> Self {
        Self {
            seconds: timestamp.as_nanosecond().div_euclid(1_000_000_000) as i64,
            nanoseconds: timestamp.as_nanosecond().rem_euclid(1_000_000_000) as u32,
        }
    }
}

pub(super) fn parse_pattern_grant(
    g: &permissions_types::PatternGrant,
) -> Result<PermissionPattern, permissions_types::PermissionError> {
    parse_permission_fields(&g.class, &g.owner, &g.recipient, &g.verb, &g.resource_id)
        .map_err(permission_error_from_parse)
}

pub(super) async fn normalize_tool_grants<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    sets: [&mut Vec<permissions_types::PatternGrant>; 4],
) -> anyhow::Result<Result<(), permissions_types::PermissionError>> {
    use golem_common::model::card::ToolResourcePattern;
    use golem_common::model::card::owner::ToolOwnerPattern;

    let mut definitions = std::collections::BTreeMap::new();
    for grant in sets.into_iter().flatten() {
        if grant.class != "tool" {
            continue;
        }
        let mut pattern = match parse_polymorphic_permission_fields(
            &grant.class,
            &grant.owner,
            &grant.recipient,
            &grant.verb,
            "*",
        ) {
            Ok(PolymorphicPermissionPattern::Tool(pattern)) => pattern,
            Ok(_) => unreachable!("tool class was checked"),
            Err(error) => return Ok(Err(permission_error_from_parse(error))),
        };
        let tool_name = match &pattern.owner {
            golem_common::model::card::owner::PolymorphicToolOwnerPattern::Concrete(
                ToolOwnerPattern::Tool {
                    account,
                    application,
                    environment,
                    component,
                    tool,
                },
            ) if account == &ctx.owner_component_metadata().account_email
                && application == &ctx.owner_component_metadata().application_name
                && environment == &ctx.owner_component_metadata().environment_name
                && component == &ctx.owner_component_metadata().component_name =>
            {
                Some(tool.clone())
            }
            _ => None,
        };
        // Discovery records the selected deployment and dynamic observations. Replaying a
        // derivation must not reinterpret aliases using the deployment that is current now.
        // All four bounds use the same observation for each tool, including an absent tool.
        let definition = match tool_name {
            Some(name) if grant.resource_id != "*" => {
                if !definitions.contains_key(&name) {
                    definitions.insert(name.clone(), ctx.get_tool_model(name.clone()).await?);
                }
                definitions[&name].as_ref()
            }
            _ => None,
        };
        pattern.resource = match ToolResourcePattern::parse_with_definition(
            &grant.resource_id,
            definition.map(|tool| &tool.definition),
        ) {
            Ok(resource) => resource,
            Err(error) => {
                return Ok(Err(permissions_types::PermissionError::InvalidPattern(
                    error.to_string(),
                )));
            }
        };
        *grant = match polymorphic_pattern_grant(&PolymorphicPermissionPattern::Tool(pattern)) {
            Ok(grant) => grant,
            Err(error) => return Ok(Err(error)),
        };
    }
    Ok(Ok(()))
}

pub(super) fn permission_error_from_parse(
    error: CardParseError,
) -> permissions_types::PermissionError {
    match error {
        CardParseError::UnknownClass(class) => {
            permissions_types::PermissionError::UnknownResourceClass(class)
        }
        CardParseError::UnknownVerb { verb, .. } => {
            permissions_types::PermissionError::InvalidVerb(verb)
        }
        CardParseError::InvalidOwnerPath { owner, .. } => {
            permissions_types::PermissionError::InvalidOwner(owner)
        }
        CardParseError::InvalidRecipientPath(recipient) => {
            permissions_types::PermissionError::InvalidRecipient(recipient)
        }
        CardParseError::Malformed(message) => {
            permissions_types::PermissionError::InvalidPattern(message)
        }
        CardParseError::InvalidResource { resource, .. } => {
            permissions_types::PermissionError::InvalidPattern(resource)
        }
        CardParseError::SlotVariableInConcreteGrant(value) => {
            permissions_types::PermissionError::InvalidPattern(value)
        }
    }
}

pub(super) fn pattern_grant_from_fields(
    fields: RenderedPermissionFields,
) -> permissions_types::PatternGrant {
    permissions_types::PatternGrant {
        class: fields.class.to_string(),
        owner: fields.owner,
        recipient: fields.recipient,
        verb: fields.verb,
        resource_id: fields.resource,
    }
}

pub(super) fn concrete_pattern_grant(
    pattern: &PermissionPattern,
) -> Result<permissions_types::PatternGrant, permissions_types::PermissionError> {
    pattern
        .render_fields()
        .map(pattern_grant_from_fields)
        .map_err(permissions_types::PermissionError::InvalidPattern)
}

pub(super) fn polymorphic_pattern_grant(
    pattern: &PolymorphicPermissionPattern,
) -> Result<permissions_types::PatternGrant, permissions_types::PermissionError> {
    pattern
        .render_fields()
        .map(pattern_grant_from_fields)
        .map_err(permissions_types::PermissionError::InvalidPattern)
}

pub(super) fn concrete_pattern_grants(
    patterns: &[PermissionPattern],
) -> Result<Vec<permissions_types::PatternGrant>, permissions_types::PermissionError> {
    patterns.iter().map(concrete_pattern_grant).collect()
}

pub(super) fn polymorphic_pattern_grants(
    patterns: &[PolymorphicPermissionPattern],
) -> Result<Vec<permissions_types::PatternGrant>, permissions_types::PermissionError> {
    patterns.iter().map(polymorphic_pattern_grant).collect()
}

pub(super) fn card_view(
    card: &ResolvedPermissionCard,
) -> Result<permissions_types::CardView, permissions_types::PermissionError> {
    match card {
        ResolvedPermissionCard::Persistent(StoredCard::Concrete(card)) => {
            Ok(permissions_types::CardView {
                lower_positive: concrete_pattern_grants(&card.lower_positive)?,
                lower_negative: concrete_pattern_grants(&card.lower_negative)?,
                upper_positive: concrete_pattern_grants(&card.upper_positive)?,
                upper_negative: concrete_pattern_grants(&card.upper_negative)?,
            })
        }
        ResolvedPermissionCard::Persistent(StoredCard::Polymorphic(card)) => {
            Ok(permissions_types::CardView {
                lower_positive: polymorphic_pattern_grants(&card.lower_positive)?,
                lower_negative: polymorphic_pattern_grants(&card.lower_negative)?,
                upper_positive: polymorphic_pattern_grants(&card.upper_positive)?,
                upper_negative: polymorphic_pattern_grants(&card.upper_negative)?,
            })
        }
        ResolvedPermissionCard::Scope(card) => Ok(permissions_types::CardView {
            lower_positive: concrete_pattern_grants(&card.lower_positive)?,
            lower_negative: concrete_pattern_grants(&card.lower_negative)?,
            upper_positive: concrete_pattern_grants(&card.upper_positive)?,
            upper_negative: concrete_pattern_grants(&card.upper_negative)?,
        }),
    }
}

pub(super) fn card_verb_name(verb: CardVerb) -> &'static str {
    match verb {
        CardVerb::Derive => "derive",
        CardVerb::Revoke => "revoke",
        CardVerb::Inspect => "inspect",
        CardVerb::Install => "install",
    }
}

pub(super) fn permission_error_from_algebra(
    error: CardAlgebraError,
) -> permissions_types::PermissionError {
    let message = error.to_string();
    permission_error_with_message(error, message)
}

/// Maps a failed wallet derivation. Its message also says what the request must restate,
/// because a wallet derivation inherits nothing from the wallet card.
pub(super) fn wallet_permission_error_from_algebra(
    error: CardAlgebraError,
) -> permissions_types::PermissionError {
    let message = error.wallet_derivation_message();
    permission_error_with_message(error, message)
}

fn permission_error_with_message(
    error: CardAlgebraError,
    message: String,
) -> permissions_types::PermissionError {
    match error {
        CardAlgebraError::InvalidOwnerPath(owner) => {
            permissions_types::PermissionError::InvalidOwner(owner)
        }
        CardAlgebraError::InvalidRecipientPath(recipient) => {
            permissions_types::PermissionError::InvalidRecipient(recipient)
        }
        CardAlgebraError::ToolPattern(message) => {
            permissions_types::PermissionError::InvalidPattern(message)
        }
        CardAlgebraError::LowerBoundTooBroad { .. }
        | CardAlgebraError::LowerDenialNotKept { .. } => {
            permissions_types::PermissionError::LowerBoundTooBroad(message)
        }
        CardAlgebraError::UpperDenialNotKept { .. }
        | CardAlgebraError::UpperCeilingNotKept { .. }
        | CardAlgebraError::UpperBoundTooBroad { .. } => {
            permissions_types::PermissionError::UpperBoundTooBroad(message)
        }
        CardAlgebraError::NoWalletCardFits { combined, .. } => {
            permission_error_with_message(*combined, message)
        }
    }
}

pub(super) fn invalid_lifespan_error(
    message: impl std::fmt::Display,
) -> permissions_types::PermissionError {
    permissions_types::PermissionError::InvalidPattern(format!(
        "invalid ISO-8601 lifespan: {message}"
    ))
}

pub(super) fn parse_lifespan(
    lifespan: Option<&permissions_types::Duration>,
) -> Result<Option<jiff::SignedDuration>, permissions_types::PermissionError> {
    let Some(lifespan) = lifespan else {
        return Ok(None);
    };
    if !lifespan.iso_8601.starts_with('P') {
        return Err(invalid_lifespan_error(
            "duration must start with the 'P' designator",
        ));
    }

    let duration = golem_schema::schema::canonical::duration::from_text(&lifespan.iso_8601)
        .map_err(invalid_lifespan_error)?;
    Ok(Some(jiff::SignedDuration::from_nanos(duration.nanoseconds)))
}

pub(super) fn derive_expires_at(
    now: jiff::Timestamp,
    lifespan: Option<&permissions_types::Duration>,
    parent_expires_at: Option<jiff::Timestamp>,
) -> Result<Option<jiff::Timestamp>, permissions_types::PermissionError> {
    if parent_expires_at.is_some_and(|expires_at| expires_at <= now) {
        return Err(permissions_types::PermissionError::CardExpired(
            "cannot derive from an expired parent card".to_string(),
        ));
    }

    let lifespan = parse_lifespan(lifespan)?;
    let Some(lifespan) = lifespan else {
        return if parent_expires_at.is_some() {
            Err(permissions_types::PermissionError::CardExpired(
                "an indefinite child card would outlive its parent card".to_string(),
            ))
        } else {
            Ok(None)
        };
    };
    let expires_at = now
        .checked_add(lifespan)
        .map_err(|_| invalid_lifespan_error("resulting expiry is out of range"))?;

    if parent_expires_at.is_some_and(|parent_expires_at| expires_at > parent_expires_at) {
        return Err(permissions_types::PermissionError::CardExpired(
            "child card expiry would exceed its parent card expiry".to_string(),
        ));
    }

    Ok(Some(expires_at))
}

#[cfg(test)]
pub(super) fn parse_grants(
    grants: &[permissions_types::PatternGrant],
) -> Result<Vec<PermissionPattern>, permissions_types::PermissionError> {
    grants.iter().map(parse_pattern_grant).collect()
}
