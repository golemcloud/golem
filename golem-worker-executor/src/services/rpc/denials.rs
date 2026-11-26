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

//! The denial renderer of host functions: the `rpc-error` that a guest gets (5.3 Rule 21).
//!
//! No host function uses it yet. The owner that wires `decide` into the host functions also
//! sends the host's real "not found" answers through [`DenialRenderer::unknown`], so that a
//! denial and an unknown ID give the same answer.

use super::RpcError;
use golem_common::model::authority::{DesignatedKind, GrantText};
use golem_common::model::security_scheme::SecuritySchemeId;
use golem_service_base::model::auth::denial::{DenialRenderer, FORBIDDEN_MESSAGE, UnknownId};

/// `rpc-error` has no field for a grant, so a denial names none: Rule 21 only limits when a
/// missing grant may be named.
pub struct HostDenials;

impl DenialRenderer for HostDenials {
    type Response = RpcError;

    /// An agent keeps the text of the host's own "not found" answer for an agent call.
    fn unknown(&self, id: &UnknownId) -> RpcError {
        let details = match id.kind() {
            DesignatedKind::Agent => format!("Worker {} not found", id.id()),
            _ => id.message(),
        };
        RpcError::NotFound { details }
    }

    fn forbidden(&self, _missing_grant: Option<&GrantText>) -> RpcError {
        RpcError::Denied {
            details: FORBIDDEN_MESSAGE.to_string(),
        }
    }

    fn sign_in_required(&self, _scheme: Option<&SecuritySchemeId>) -> RpcError {
        RpcError::Denied {
            details: "Sign-in required".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::AgentId;
    use golem_common::model::authority::{Denial, DesignatedKind};
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use golem_service_base::model::auth::denial::FORBIDDEN_MESSAGE;
    use test_r::test;

    #[test]
    fn a_host_denial_answers_as_a_call_to_an_unknown_agent() {
        let agent_id = AgentId::from_parts(7, 3);
        let real = RpcError::from(WorkerExecutorError::AgentNotFound { agent_id });
        assert_eq!(
            HostDenials.render(
                &Denial::LooksUnknown {
                    kind: DesignatedKind::Agent
                },
                &[UnknownId::new(DesignatedKind::Agent, agent_id)]
            ),
            real
        );
    }

    #[test]
    fn every_other_designated_kind_is_not_found_with_the_shared_text() {
        for kind in [
            DesignatedKind::Invocation,
            DesignatedKind::Task,
            DesignatedKind::Stream,
            DesignatedKind::Promise,
            DesignatedKind::Webhook,
            DesignatedKind::Artifact,
        ] {
            let id = UnknownId::new(kind, "id-7");
            assert_eq!(
                HostDenials.render(&Denial::LooksUnknown { kind }, std::slice::from_ref(&id)),
                RpcError::NotFound {
                    details: id.message()
                },
                "{kind:?}"
            );
        }
    }

    #[test]
    fn a_host_forbidden_answer_names_no_target_and_no_grant() {
        for missing_grant in [None, Some(GrantText("tool(E/grep) : invoke".to_string()))] {
            assert_eq!(
                HostDenials.render(&Denial::Forbidden { missing_grant }, &[]),
                RpcError::Denied {
                    details: FORBIDDEN_MESSAGE.to_string()
                }
            );
        }
    }
}
