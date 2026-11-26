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

//! How a denial looks to the caller.
//!
//! A denial on a target that a request designates by ID (an agent, an invocation, a task, a
//! stream, a promise, a webhook or an artifact) is byte-identical to an unknown ID on every
//! surface. Every other denial is a `403`, which names the missing grant only when the
//! authorizer gives one, and no message holds a target's account or a claim.
//!
//! Each surface implements [`DenialRenderer`] once. The surface's real "not found" answer for
//! a designated kind and its denials both come from [`DenialRenderer::unknown`], built from the
//! same [`UnknownId`], so the two answers cannot drift apart.

use golem_common::base_model::api::error_code;
use golem_common::model::authority::{Denial, DesignatedKind, GrantText};
use golem_common::model::security_scheme::SecuritySchemeId;

/// The text of every `403`. It names no target, account or claim.
pub const FORBIDDEN_MESSAGE: &str = "Access denied";

/// The ID of a designated target, in the text form in which the surface shows that ID in its
/// "not found" answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownId {
    kind: DesignatedKind,
    id: String,
}

impl UnknownId {
    pub fn new(kind: DesignatedKind, id: impl ToString) -> Self {
        Self {
            kind,
            id: id.to_string(),
        }
    }

    pub fn kind(&self) -> DesignatedKind {
        self.kind
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// The error code of "not found" for this kind, the same on every surface.
    pub fn code(&self) -> &'static str {
        match self.kind {
            DesignatedKind::Agent => error_code::AGENT_NOT_FOUND,
            DesignatedKind::Invocation => error_code::INVOCATION_NOT_FOUND,
            DesignatedKind::Task => error_code::TASK_NOT_FOUND,
            DesignatedKind::Stream => error_code::STREAM_NOT_FOUND,
            DesignatedKind::Promise => error_code::PROMISE_NOT_FOUND,
            DesignatedKind::Webhook => error_code::WEBHOOK_NOT_FOUND,
            DesignatedKind::Artifact => error_code::ARTIFACT_NOT_FOUND,
        }
    }

    /// The text of "not found" for this kind. Agents, invocations and promises keep the text
    /// that their "not found" errors had before the renderers.
    pub fn message(&self) -> String {
        let noun = match self.kind {
            DesignatedKind::Agent => "Worker",
            DesignatedKind::Invocation => "Invocation",
            DesignatedKind::Task => "Task",
            DesignatedKind::Stream => "Stream",
            DesignatedKind::Promise => "Promise",
            DesignatedKind::Webhook => "Webhook",
            DesignatedKind::Artifact => "Artifact",
        };
        format!("{noun} not found: {}", self.id)
    }
}

/// Turns a denial into the response of one surface.
pub trait DenialRenderer {
    type Response;

    /// The answer for an ID that names nothing. The surface's real "not found" path calls this
    /// too.
    fn unknown(&self, id: &UnknownId) -> Self::Response;

    /// A `403` with [`FORBIDDEN_MESSAGE`]. It names `missing_grant` when the authorizer gives
    /// one, which it does only for a target that the principal may view.
    fn forbidden(&self, missing_grant: Option<&GrantText>) -> Self::Response;

    /// The answer when signing in could admit the request.
    fn sign_in_required(&self, scheme: Option<&SecuritySchemeId>) -> Self::Response;

    /// Renders `denial`. `designated` holds the IDs that the request names, at most one per
    /// kind; `LooksUnknown` takes the one of its kind. Without it, a debug build stops on an
    /// assertion, and a release build still answers with the status and code of an unknown ID,
    /// so even a caller's mistake reveals no target.
    fn render(&self, denial: &Denial, designated: &[UnknownId]) -> Self::Response {
        match denial {
            Denial::LooksUnknown { kind } => {
                let id = designated.iter().find(|id| id.kind == *kind);
                if id.is_none() {
                    tracing::error!(
                        ?kind,
                        "A denial looks like an unknown ID, but the request names no ID of its kind"
                    );
                }
                debug_assert!(
                    id.is_some(),
                    "A denial looks like an unknown ID, but the request names no ID of its kind"
                );
                match id {
                    Some(id) => self.unknown(id),
                    None => self.unknown(&UnknownId::new(*kind, "")),
                }
            }
            Denial::Forbidden { missing_grant } => self.forbidden(missing_grant.as_ref()),
            Denial::SignInRequired { scheme } => self.sign_in_required(scheme.as_ref()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::auth::AuthorizationError;
    use golem_common::SafeDisplay;
    use golem_common::model::account::AccountEmail;
    use golem_common::model::authority::CatalogOperation;
    use golem_common::model::card::owner::AgentOwnerPattern;
    use golem_common::model::card::{
        AgentResourcePattern, AgentVerb, CardAlgebraError, ClassPermissionTarget, PermissionTarget,
    };
    use std::collections::BTreeSet;
    use test_r::test;

    const ALL_KINDS: [DesignatedKind; 7] = [
        DesignatedKind::Agent,
        DesignatedKind::Invocation,
        DesignatedKind::Task,
        DesignatedKind::Stream,
        DesignatedKind::Promise,
        DesignatedKind::Webhook,
        DesignatedKind::Artifact,
    ];

    /// Shows which renderer method a denial reached, and with what.
    struct Trace;

    impl DenialRenderer for Trace {
        type Response = String;

        fn unknown(&self, id: &UnknownId) -> String {
            format!("unknown {:?} {}", id.kind(), id.id())
        }

        fn forbidden(&self, missing_grant: Option<&GrantText>) -> String {
            format!("forbidden {missing_grant:?}")
        }

        fn sign_in_required(&self, scheme: Option<&SecuritySchemeId>) -> String {
            format!("sign-in {scheme:?}")
        }
    }

    #[test]
    fn agents_invocations_and_promises_keep_todays_not_found_text_and_code() {
        let agent = UnknownId::new(DesignatedKind::Agent, "a1");
        assert_eq!(agent.message(), "Worker not found: a1");
        assert_eq!(agent.code(), "AGENT_NOT_FOUND");

        let invocation = UnknownId::new(DesignatedKind::Invocation, "a1.i1");
        assert_eq!(invocation.message(), "Invocation not found: a1.i1");
        assert_eq!(invocation.code(), "INVOCATION_NOT_FOUND");

        let promise = UnknownId::new(DesignatedKind::Promise, "p1");
        assert_eq!(promise.message(), "Promise not found: p1");
        assert_eq!(promise.code(), "PROMISE_NOT_FOUND");
    }

    #[test]
    fn each_designated_kind_has_its_own_not_found_code() {
        let codes = ALL_KINDS
            .iter()
            .map(|kind| UnknownId::new(*kind, "x").code())
            .collect::<BTreeSet<_>>();
        assert_eq!(codes.len(), ALL_KINDS.len(), "{codes:?}");
        for kind in ALL_KINDS {
            let id = UnknownId::new(kind, "id-7");
            assert!(id.code().ends_with("_NOT_FOUND"), "{kind:?}: {}", id.code());
            assert!(
                id.message().ends_with(": id-7"),
                "{kind:?}: {}",
                id.message()
            );
        }
    }

    #[test]
    fn looks_unknown_renders_the_designated_id_of_its_kind() {
        let designated = [
            UnknownId::new(DesignatedKind::Invocation, "a1.i1"),
            UnknownId::new(DesignatedKind::Agent, "a1"),
        ];
        assert_eq!(
            Trace.render(
                &Denial::LooksUnknown {
                    kind: DesignatedKind::Agent
                },
                &designated
            ),
            "unknown Agent a1"
        );
        assert_eq!(
            Trace.render(
                &Denial::LooksUnknown {
                    kind: DesignatedKind::Invocation
                },
                &designated
            ),
            "unknown Invocation a1.i1"
        );
    }

    /// A wiring mistake stops tests and debug builds. A release build still answers with the
    /// status and code of an unknown ID, so even this mistake reveals no target.
    #[test]
    fn looks_unknown_without_its_id_stops_a_debug_build() {
        let rendered = std::panic::catch_unwind(|| {
            Trace.render(
                &Denial::LooksUnknown {
                    kind: DesignatedKind::Webhook,
                },
                &[UnknownId::new(DesignatedKind::Agent, "a1")],
            )
        });
        if cfg!(debug_assertions) {
            assert!(rendered.is_err(), "a debug build must stop: {rendered:?}");
        } else {
            assert_eq!(rendered.unwrap(), "unknown Webhook ");
        }
    }

    #[test]
    fn forbidden_passes_the_missing_grant_only_when_the_authorizer_gives_one() {
        assert_eq!(
            Trace.render(
                &Denial::Forbidden {
                    missing_grant: None
                },
                &[]
            ),
            "forbidden None"
        );
        let grant = GrantText("tool(E/grep) : invoke".to_string());
        assert_eq!(
            Trace.render(
                &Denial::Forbidden {
                    missing_grant: Some(grant.clone())
                },
                &[]
            ),
            format!("forbidden {:?}", Some(&grant))
        );
    }

    #[test]
    fn sign_in_required_passes_its_scheme() {
        assert_eq!(
            Trace.render(&Denial::SignInRequired { scheme: None }, &[]),
            "sign-in None"
        );
    }

    /// `LooksUnknown { kind }` names exactly one part of the request only while no operation has
    /// two parts that can target the same designated kind.
    #[test]
    fn no_operation_has_two_parts_of_one_designated_kind() {
        for operation in CatalogOperation::ALL {
            let mut seen = BTreeSet::new();
            for part in operation.parts() {
                let kinds = part
                    .targets
                    .iter()
                    .filter_map(|target| target.designated_kind())
                    .map(|kind| format!("{kind:?}"))
                    .collect::<BTreeSet<_>>();
                for kind in kinds {
                    assert!(
                        seen.insert(kind.clone()),
                        "{} has two parts that can target {kind}",
                        operation.name()
                    );
                }
            }
        }
    }

    fn target_of_another_account() -> Box<PermissionTarget> {
        Box::new(PermissionTarget::Agent(ClassPermissionTarget {
            owner: AgentOwnerPattern::AccountAgents {
                account: AccountEmail::new("owner@example.com"),
            },
            verb: Some(AgentVerb::Invoke),
            resource: AgentResourcePattern::Any,
        }))
    }

    #[test]
    fn a_denied_permission_shows_no_target_and_no_account() {
        for error in [
            AuthorizationError::PermissionNotAllowed(target_of_another_account()),
            AuthorizationError::PermissionEvaluationFailed {
                target: target_of_another_account(),
                error: CardAlgebraError::InvalidOwnerPath("owner@example.com/app".to_string()),
            },
        ] {
            assert_eq!(error.to_safe_string(), FORBIDDEN_MESSAGE, "{error:?}");
            assert_eq!(error.to_string(), FORBIDDEN_MESSAGE, "{error:?}");
            assert!(
                format!("{error:?}").contains("owner@example.com"),
                "the debug form keeps the target for logs: {error:?}"
            );
        }
    }
}
