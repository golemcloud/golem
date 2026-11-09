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

use super::Kind;
use crate::base_model::AgentFingerprint;
use crate::base_model::account::AccountId;
use crate::base_model::environment::EnvironmentId;
use crate::base_model::tool::ToolName;
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use uuid::Uuid;

/// The subject of an agent, an account or a tool under a Golem issuer.
///
/// Each subject starts with its kind, so subjects of different kinds never collide. Every ID is a
/// lowercase UUID with hyphens, and the parser accepts only the text that [`Display`] writes, so
/// one principal has exactly one subject.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GolemSubject {
    /// `agent:<agent-id>/<fingerprint>`. The agent ID is the agent's 2.0 ID, never its key. An
    /// agent created again under the same ID gets a new fingerprint, so it is a new pair.
    Agent {
        agent_id: Uuid,
        fingerprint: AgentFingerprint,
    },
    /// `account:<account-id>`, never the account's path, so a rename keeps the pair.
    Account { account_id: AccountId },
    /// `tool:<environment-id>/<tool-name>`. It holds nothing of one declaration, so a tool that
    /// is declared again under the same name keeps its pair.
    Tool {
        environment_id: EnvironmentId,
        tool_name: ToolName,
    },
}

impl GolemSubject {
    pub fn kind(&self) -> Kind {
        match self {
            GolemSubject::Agent { .. } => Kind::Agent,
            GolemSubject::Account { .. } => Kind::Account,
            GolemSubject::Tool { .. } => Kind::Tool,
        }
    }
}

impl Display for GolemSubject {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            GolemSubject::Agent {
                agent_id,
                fingerprint,
            } => write!(
                f,
                "agent:{}/{}",
                agent_id.hyphenated(),
                fingerprint.0.hyphenated()
            ),
            GolemSubject::Account { account_id } => {
                write!(f, "account:{}", account_id.0.hyphenated())
            }
            GolemSubject::Tool {
                environment_id,
                tool_name,
            } => write!(f, "tool:{}/{}", environment_id.0.hyphenated(), tool_name),
        }
    }
}

impl FromStr for GolemSubject {
    type Err = String;

    /// Reads a subject that [`Display`] wrote, and refuses every other spelling.
    fn from_str(subject: &str) -> Result<Self, Self::Err> {
        let (kind, rest) = subject
            .split_once(':')
            .ok_or_else(|| format!("invalid Golem subject {subject:?}: no kind prefix"))?;
        let parsed = match kind {
            "agent" => {
                let (agent_id, fingerprint) = rest.split_once('/').ok_or_else(|| {
                    format!(
                        "invalid agent subject {subject:?}: expected agent:<agent-id>/<fingerprint>"
                    )
                })?;
                GolemSubject::Agent {
                    agent_id: canonical_uuid(agent_id, subject)?,
                    fingerprint: AgentFingerprint(canonical_uuid(fingerprint, subject)?),
                }
            }
            "account" => GolemSubject::Account {
                account_id: AccountId(canonical_uuid(rest, subject)?),
            },
            "tool" => {
                let (environment_id, tool_name) = rest.split_once('/').ok_or_else(|| {
                    format!(
                        "invalid tool subject {subject:?}: expected tool:<environment-id>/<tool-name>"
                    )
                })?;
                GolemSubject::Tool {
                    environment_id: EnvironmentId(canonical_uuid(environment_id, subject)?),
                    tool_name: ToolName::try_from(tool_name)
                        .map_err(|error| format!("invalid tool subject {subject:?}: {error}"))?,
                }
            }
            other => {
                return Err(format!(
                    "invalid Golem subject {subject:?}: unknown kind {other:?}"
                ));
            }
        };
        Ok(parsed)
    }
}

/// Reads a UUID in its one subject spelling: lowercase hex with hyphens.
fn canonical_uuid(text: &str, subject: &str) -> Result<Uuid, String> {
    match Uuid::parse_str(text) {
        Ok(uuid) if uuid.hyphenated().to_string() == text => Ok(uuid),
        _ => Err(format!(
            "invalid Golem subject {subject:?}: {text:?} is not a lowercase UUID with hyphens"
        )),
    }
}
