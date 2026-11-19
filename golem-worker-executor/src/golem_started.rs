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

//! The callers of work that Golem starts itself, rather than a request or another agent. No
//! invocation runs without a caller, so every Golem-started invocation gets its caller from
//! [`GolemStartedWork::caller`], and no executor path builds an anonymous caller for such work.

use golem_common::model::account::AccountId;
use golem_common::model::agent::{AgentTypeName, OwnerKind};
use golem_common::model::caller::{
    AccountAuthentication, AccountPrincipal, AgentPrincipal, Caller, Principal,
};
use golem_common::model::golem_issuer::GolemIssuer;
use golem_common::model::{AgentMetadata, OwnedAgentId};

/// Work that Golem starts itself.
pub(crate) enum GolemStartedWork {
    /// The agent runs code for itself, such as a snapshot save or load: the agent in both roles.
    SelfInvocation { agent: Principal },
    /// An oplog processor processes a batch of another agent's oplog: the account that installed
    /// the processor is the original caller, and the agent whose oplog it processes is the
    /// immediate caller.
    OplogProcessorBatch {
        issuer: GolemIssuer,
        installed_by: AccountId,
        processed_agent: Principal,
    },
}

impl GolemStartedWork {
    pub(crate) fn caller(self) -> Caller {
        match self {
            Self::SelfInvocation { agent } => Caller::entry(agent),
            Self::OplogProcessorBatch {
                issuer,
                installed_by,
                processed_agent,
            } => Caller {
                // The same account pair that a request with the account's token gets.
                original: Principal::Account(AccountPrincipal {
                    issuer: issuer.to_string(),
                    account_id: installed_by,
                    authentication: AccountAuthentication::ApiToken,
                }),
                immediate: processed_agent,
                tokens: Vec::new(),
            },
        }
    }
}

/// An agent as a principal, under the Golem issuer. An owner that has no agent type gets the
/// fixed type name of its owner kind. It is the only builder of an agent principal in the
/// executor.
pub(crate) fn agent_principal(
    issuer: &GolemIssuer,
    owned_agent_id: &OwnedAgentId,
    metadata: &AgentMetadata,
) -> Principal {
    let agent_type = match (&metadata.agent_type_name, metadata.owner_kind) {
        (Some(agent_type), _) => agent_type.clone(),
        (None, OwnerKind::EphemeralExternalTool) => {
            AgentTypeName("~golem-external-tool-owner".to_string())
        }
        (None, OwnerKind::OplogProcessor) => AgentTypeName("~golem-oplog-processor".to_string()),
        (None, OwnerKind::ComponentAgent) => AgentTypeName(String::new()),
    };
    Principal::Agent(AgentPrincipal::of_agent(
        issuer.to_string(),
        owned_agent_id.agent_id.clone(),
        metadata.fingerprint,
        agent_type,
        owned_agent_id.environment_id,
    ))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use test_r::test;

    /// Production code that may build an anonymous caller, and why. Everything else gets its
    /// caller from the request, from the invocation that makes a call, or from
    /// [`super::GolemStartedWork`].
    const ANONYMOUS_CALLERS: &[(&str, &str)] = &[
        (
            "grpc/invocation.rs",
            "a card transfer request comes from the sending agent's executor with no caller",
        ),
        (
            "services/worker_proxy.rs",
            "an invocation status lookup starts no work",
        ),
    ];

    #[test]
    fn only_entry_points_build_an_anonymous_caller() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        for file in rust_files(&src) {
            let relative = file
                .strip_prefix(&src)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if is_test_file(&relative) {
                continue;
            }
            let source = std::fs::read_to_string(&file).unwrap();
            if production_code(&source).iter().any(|line| {
                line.contains("Caller::anonymous()") || line.contains("Principal::Anonymous")
            }) {
                found.push(relative);
            }
        }
        found.sort();
        let mut allowed: Vec<String> = ANONYMOUS_CALLERS
            .iter()
            .map(|(file, _)| file.to_string())
            .collect();
        allowed.sort();
        assert_eq!(
            found, allowed,
            "Golem-started work must get its caller from GolemStartedWork::caller"
        );
    }

    fn rust_files(dir: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.extend(rust_files(&path));
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
        files
    }

    fn is_test_file(relative: &str) -> bool {
        let name = relative.rsplit('/').next().unwrap();
        relative.split('/').any(|part| part == "tests")
            || name == "tests.rs"
            || name.ends_with("_tests.rs")
            || name.starts_with("test_")
            || name.contains("test_support")
            || name.ends_with("_testing.rs")
    }

    /// The lines of a source file outside the items marked `#[cfg(test)]` or
    /// `#[cfg(any(test, ...))]`.
    fn production_code(source: &str) -> Vec<&str> {
        let mut lines = source.lines();
        let mut result = Vec::new();
        while let Some(line) = lines.next() {
            let trimmed = line.trim_start();
            if !(trimmed.starts_with("#[cfg(test)]") || trimmed.starts_with("#[cfg(any(test")) {
                result.push(line);
                continue;
            }
            let mut depth = 0i64;
            let mut opened = false;
            for item_line in lines.by_ref() {
                if !opened && item_line.trim_start().starts_with("#[") {
                    continue;
                }
                depth += braces(item_line);
                opened |= item_line.contains('{');
                if (opened && depth <= 0) || (!opened && item_line.trim_end().ends_with(';')) {
                    break;
                }
            }
        }
        result
    }

    /// The change in brace depth over a line, ignoring braces in string and char literals and in
    /// line comments.
    fn braces(line: &str) -> i64 {
        let mut depth = 0;
        let mut in_string = false;
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' if in_string => {
                    chars.next();
                }
                '"' => in_string = !in_string,
                '\'' if !in_string => {
                    // A char literal such as '{' or '\''; a lifetime has no closing quote.
                    let rest: String = chars.clone().take(3).collect();
                    if rest.starts_with('\\') {
                        chars.nth(2);
                    } else if rest.chars().nth(1) == Some('\'') {
                        chars.nth(1);
                    }
                }
                '/' if !in_string && chars.peek() == Some(&'/') => break,
                '{' if !in_string => depth += 1,
                '}' if !in_string => depth -= 1,
                _ => {}
            }
        }
        depth
    }
}
