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

//! A decision answered from today's card checks. Call sites build the decision request of the
//! decision API, and pass beside it the card checks that they make today, so that moving a check
//! to `decide` changes no answer. The switch to the new card model replaces this module.

use super::owner::AgentOwnerPattern;
use super::{
    AccountTokenVerb, AccountVerb, AgentResourcePattern, AgentVerb, ApplicationVerb,
    CardAlgebraError, CardId, CardVerb, ClassPermissionTarget, EffectiveSurface,
    EnvironmentAgentSecretVerb, EnvironmentDomainRegistrationVerb, EnvironmentVerb, FilesystemVerb,
    OplogVerb, PermissionTarget, SystemVerb, ToolVerb,
};
use crate::base_model::authority::{
    self as decision, Decision, Denial, GrantorCard, Grantors, Operation, OperationPart, Verb,
};

/// Decides `operation` from today's checks. `checks[i]` holds the card checks that the call site
/// makes today for part `i` of the operation.
///
/// A part is allowed when each of its checks belongs to the part (`check_belongs_to_part`) and
/// `surface` allows it. A part with no check is allowed: the entry point checks nothing for it
/// today. The operation is allowed when every part is allowed; otherwise the denial is the denial
/// of the first part that fails, in the operation's order. An error inside the evaluation, or a
/// check list that does not match the parts, denies.
pub fn decide_with_checks(
    surface: &EffectiveSurface,
    operation: &Operation,
    checks: &[Vec<PermissionTarget>],
) -> Decision {
    let parts = operation.parts();
    if checks.len() != parts.len() {
        return Decision::Deny(parts.first().map(denial).unwrap_or(Denial::Forbidden {
            missing_grant: None,
        }));
    }

    let mut cards: Vec<CardId> = Vec::new();
    for (part, part_checks) in parts.iter().zip(checks) {
        match allowing_cards(surface, part.verb, part_checks) {
            Ok(Some(allowing)) => {
                for card in allowing {
                    if !cards.contains(&card) {
                        cards.push(card);
                    }
                }
            }
            Ok(None) | Err(_) => return Decision::Deny(denial(part)),
        }
    }

    Decision::Allow(Grantors {
        cards: cards
            .into_iter()
            .map(|card_id| GrantorCard {
                card_id,
                ancestors: Vec::new(),
                presented_by: None,
            })
            .collect(),
    })
}

/// The card check that answers `agent : exec` on an agent: the `invoke : *` authority on the same
/// agent, which admitted hosted tool calls before `exec` existed.
pub fn exec_check(owner: AgentOwnerPattern) -> PermissionTarget {
    PermissionTarget::Agent(ClassPermissionTarget {
        owner,
        verb: Some(AgentVerb::Invoke),
        resource: AgentResourcePattern::Any,
    })
}

/// Whether `check` is a card check that today's code makes for an operation part with `verb`.
/// It keeps a call site from answering one operation with the check of another.
pub fn check_belongs_to_part(verb: Verb, check: &PermissionTarget) -> bool {
    use PermissionTarget as Check;

    match (verb, check) {
        (Verb::Agent(decision::AgentVerb::Exec), Check::Agent(check)) => {
            check.verb == Some(AgentVerb::Invoke) && check.resource == AgentResourcePattern::Any
        }
        (Verb::Agent(verb), Check::Agent(check)) => {
            agent_verb(verb).is_some() && check.verb == agent_verb(verb)
        }
        (Verb::Oplog(decision::OplogVerb::Read), Check::Oplog(check)) => {
            check.verb == Some(OplogVerb::Read)
        }
        (Verb::Filesystem(decision::FilesystemVerb::List), Check::Filesystem(check)) => {
            matches!(
                check.verb,
                Some(FilesystemVerb::Stat | FilesystemVerb::List)
            )
        }
        (Verb::Filesystem(decision::FilesystemVerb::Read), Check::Filesystem(check)) => {
            check.verb == Some(FilesystemVerb::Read)
        }
        (Verb::Invocation(decision::InvocationVerb::Cancel), Check::Agent(check)) => {
            check.verb == Some(AgentVerb::CancelInvocation)
        }
        (Verb::Invocation(decision::InvocationVerb::Read), Check::Agent(check)) => {
            check.verb == Some(AgentVerb::View)
        }
        (Verb::Promise(decision::PromiseVerb::Send), Check::Agent(check)) => {
            check.verb == Some(AgentVerb::Invoke)
        }
        (Verb::Tool(decision::ToolVerb::Invoke), Check::Agent(check)) => {
            check.verb == Some(AgentVerb::Invoke)
        }
        (Verb::Tool(decision::ToolVerb::Invoke), Check::Tool(check)) => {
            check.verb == Some(ToolVerb::Invoke)
        }
        (Verb::Account(verb), Check::Account(check)) => {
            verb == decision::AccountVerb::Update && check.verb == Some(AccountVerb::Update)
        }
        (Verb::Account(verb), Check::System(check)) => match verb {
            decision::AccountVerb::Create => check.verb == Some(SystemVerb::CreateAccount),
            decision::AccountVerb::ViewAccountSummariesReport => {
                check.verb == Some(SystemVerb::ViewAccountSummariesReport)
            }
            decision::AccountVerb::ViewAccountCountsReport => {
                check.verb == Some(SystemVerb::ViewAccountCountsReport)
            }
            _ => false,
        },
        (Verb::Account(verb), Check::AccountToken(check)) => match verb {
            decision::AccountVerb::CreateApiTokens => check.verb == Some(AccountTokenVerb::Create),
            decision::AccountVerb::ViewApiTokens => check.verb == Some(AccountTokenVerb::View),
            decision::AccountVerb::RevokeApiTokens => matches!(
                check.verb,
                Some(AccountTokenVerb::View | AccountTokenVerb::Delete)
            ),
            _ => false,
        },
        (Verb::Application(decision::ApplicationVerb::Create), Check::Application(check)) => {
            check.verb == Some(ApplicationVerb::Create)
        }
        (Verb::Environment(verb), Check::Environment(check)) => match verb {
            decision::EnvironmentVerb::Create => check.verb == Some(EnvironmentVerb::Create),
            decision::EnvironmentVerb::Deploy => check.verb == Some(EnvironmentVerb::Deploy),
            _ => false,
        },
        (
            Verb::EnvironmentDomainRegistration(
                decision::EnvironmentDomainRegistrationVerb::Create,
            ),
            Check::EnvironmentDomainRegistration(check),
        ) => check.verb == Some(EnvironmentDomainRegistrationVerb::Create),
        (Verb::Secret(verb), Check::EnvironmentAgentSecret(check)) => {
            secret_verb(verb).is_some() && check.verb == secret_verb(verb)
        }
        (Verb::Card(verb), Check::Card(check)) => match verb {
            decision::CardVerb::Inspect => check.verb == Some(CardVerb::Inspect),
            decision::CardVerb::Revoke => check.verb == Some(CardVerb::Revoke),
            _ => false,
        },
        _ => false,
    }
}

/// Today's agent verb for an agent part.
fn agent_verb(verb: decision::AgentVerb) -> Option<AgentVerb> {
    use decision::AgentVerb as Part;

    match verb {
        Part::Create | Part::Invoke => Some(AgentVerb::Invoke),
        Part::View => Some(AgentVerb::View),
        Part::Delete => Some(AgentVerb::Delete),
        Part::Interrupt => Some(AgentVerb::Interrupt),
        Part::Resume => Some(AgentVerb::Resume),
        Part::UpdateRevision => Some(AgentVerb::UpdateRevision),
        Part::Fork => Some(AgentVerb::Fork),
        Part::Revert => Some(AgentVerb::Revert),
        Part::ActivateProcessor => Some(AgentVerb::ActivatePlugin),
        Part::DeactivateProcessor => Some(AgentVerb::DeactivatePlugin),
        _ => None,
    }
}

/// Today's environment secret verb for a secret part.
fn secret_verb(verb: decision::SecretVerb) -> Option<EnvironmentAgentSecretVerb> {
    use decision::SecretVerb as Part;

    match verb {
        Part::View => Some(EnvironmentAgentSecretVerb::View),
        Part::Create => Some(EnvironmentAgentSecretVerb::Create),
        Part::Update => Some(EnvironmentAgentSecretVerb::Update),
        Part::Delete => Some(EnvironmentAgentSecretVerb::Delete),
        _ => None,
    }
}

/// Whether today's `checks` allow a part with `verb`, as `decide_with_checks` decides one part.
/// Listings use it for each row and for each agent type, which have no operation of their own.
pub fn checks_allow(surface: &EffectiveSurface, verb: Verb, checks: &[PermissionTarget]) -> bool {
    matches!(allowing_cards(surface, verb, checks), Ok(Some(_)))
}

/// The cards that allow every check of a part with `verb`, or `None` when one check does not
/// belong to the part or is not allowed.
fn allowing_cards(
    surface: &EffectiveSurface,
    verb: Verb,
    checks: &[PermissionTarget],
) -> Result<Option<Vec<CardId>>, CardAlgebraError> {
    let mut cards = Vec::new();
    for check in checks {
        if !check_belongs_to_part(verb, check) || !surface.authorize(check)? {
            return Ok(None);
        }
        cards.extend(allowing_card(surface, check)?);
    }
    Ok(Some(cards))
}

/// The card whose grant allows `check`. A surface built from cards keeps one lower bound per card,
/// in the order of `source_card_ids`; a surface built from loose grants names no card.
fn allowing_card(
    surface: &EffectiveSurface,
    check: &PermissionTarget,
) -> Result<Option<CardId>, CardAlgebraError> {
    if surface.source_card_ids.len() != surface.lower.len() {
        return Ok(None);
    }
    for (card_id, lower) in surface.source_card_ids.iter().zip(&surface.lower) {
        if lower.allows(check)? {
            return Ok(Some(*card_id));
        }
    }
    Ok(None)
}

/// The denial of a failed part: an unknown ID for a target of a designated kind, else a `403` that
/// names no grant.
fn denial(part: &OperationPart) -> Denial {
    match part.target.kind().designated_kind() {
        Some(kind) => Denial::LooksUnknown { kind },
        None => Denial::Forbidden {
            missing_grant: None,
        },
    }
}
