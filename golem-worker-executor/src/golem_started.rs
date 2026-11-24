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
        owned_agent_id.agent_id,
        metadata.fingerprint,
        agent_type,
        owned_agent_id.environment_id,
    ))
}

#[cfg(test)]
mod tests {
    use super::GolemStartedWork;
    use golem_common::model::account::AccountId;
    use golem_common::model::agent::AgentTypeName;
    use golem_common::model::caller::{AccountAuthentication, AgentPrincipal, Kind, Principal};
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::golem_issuer::GolemIssuer;
    use golem_common::model::{AgentFingerprint, AgentId};
    use std::path::{Path, PathBuf};
    use test_r::test;
    use uuid::Uuid;

    fn issuer() -> GolemIssuer {
        "https://golem.test".parse().unwrap()
    }

    fn agent() -> Principal {
        Principal::Agent(AgentPrincipal::of_agent(
            issuer().to_string(),
            AgentId::from_parts(7, 0xc0),
            AgentFingerprint(Uuid::from_u128(0xf1)),
            AgentTypeName("Counter".to_string()),
            EnvironmentId(Uuid::from_u128(0xe0)),
        ))
    }

    #[test]
    fn a_self_invocation_has_the_agent_in_both_roles() {
        let caller = GolemStartedWork::SelfInvocation { agent: agent() }.caller();
        assert_eq!(caller.original.kind(), Kind::Agent);
        assert!(caller.original.equals(&agent()));
        assert!(caller.immediate.equals(&agent()));
        assert!(caller.tokens.is_empty());
    }

    #[test]
    fn an_oplog_processor_batch_has_the_installer_and_then_the_processed_agent() {
        let installed_by = AccountId::new();
        let caller = GolemStartedWork::OplogProcessorBatch {
            issuer: issuer(),
            installed_by,
            processed_agent: agent(),
        }
        .caller();
        // The original caller is the account pair that a request with the installer's token gets.
        match &caller.original {
            Principal::Account(account) => {
                assert_eq!(account.issuer, issuer().to_string());
                assert_eq!(account.account_id, installed_by);
                assert_eq!(account.authentication, AccountAuthentication::ApiToken);
            }
            other => panic!("the original caller must be an account, got {other:?}"),
        }
        assert!(caller.immediate.equals(&agent()));
        assert!(!caller.original.equals(&caller.immediate));
        assert!(caller.tokens.is_empty());
    }

    /// Production code that may build an anonymous caller, once per file, and why. Everything
    /// else gets its caller from the request, from the invocation that makes a call, or from
    /// [`super::GolemStartedWork`].
    const ANONYMOUS_CALLERS: &[(&str, &str)] = &[
        (
            "grpc/invocation.rs",
            "a card transfer request carries no caller, and the transfer reaches only an existing \
             recipient, so no constructor or invocation runs with this caller",
        ),
        (
            "services/worker_proxy.rs",
            "an invocation status lookup starts no work",
        ),
        (
            "worker/invocation.rs",
            "the validation of a fresh invocation builds a placeholder invocation that neither \
             registers streams nor enters a guest, so it starts no work",
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
            let count: usize = production_code(&source)
                .into_iter()
                .map(anonymous_callers)
                .sum();
            if count > 0 {
                found.push((relative, count));
            }
        }
        found.sort();
        let mut allowed: Vec<(String, usize)> = ANONYMOUS_CALLERS
            .iter()
            .map(|(file, _)| (file.to_string(), 1))
            .collect();
        allowed.sort();
        assert_eq!(
            found, allowed,
            "Golem-started work must get its caller from GolemStartedWork::caller"
        );
    }

    #[test]
    fn the_scan_counts_only_the_names_that_it_looks_for() {
        assert_eq!(anonymous_callers("let caller = Caller::anonymous();"), 1);
        assert_eq!(anonymous_callers("Principal::Anonymous => None,"), 1);
        assert_eq!(anonymous_callers("caller::Principal::Anonymous"), 1);
        assert_eq!(
            anonymous_callers(
                "PublicPrincipal::Anonymous(_) => oplog::PublicPrincipal::Anonymous,"
            ),
            0
        );
    }

    /// The anonymous callers that a line of code names. A match counts only where the name starts,
    /// so `PublicPrincipal::Anonymous`, the public form of a recorded caller, does not count.
    fn anonymous_callers(line: &str) -> usize {
        ["Caller::anonymous()", "Principal::Anonymous"]
            .iter()
            .flat_map(|name| line.match_indices(name))
            .filter(|(start, _)| {
                !line[..*start]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_')
            })
            .count()
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

    /// The lines of a source file outside the items, fields and match arms marked
    /// `#[cfg(test)]` or `#[cfg(any(test, ...))]`.
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
            let mut first = true;
            let mut literal = Literal::None;
            for item_line in lines.by_ref() {
                if !opened && item_line.trim_start().starts_with("#[") {
                    continue;
                }
                depth += braces(item_line, &mut literal);
                opened |= item_line.contains('{');
                let end = item_line.trim_end();
                // A field or a match arm ends at the comma of its first line.
                if (opened && depth <= 0)
                    || (!opened && (end.ends_with(';') || (first && end.ends_with(','))))
                {
                    break;
                }
                first = false;
            }
        }
        result
    }

    /// The literal that a line ends in, so that the next line of the same item continues it.
    #[derive(Clone, Copy, PartialEq)]
    enum Literal {
        None,
        String,
        /// A raw string, with the number of `#` that close it.
        Raw(usize),
    }

    /// The change in brace depth over a line, ignoring braces in string, raw string and char
    /// literals and in line comments. A string or raw string can span lines: `literal` holds the
    /// one that the previous line left open, and the one that this line leaves open.
    fn braces(line: &str, literal: &mut Literal) -> i64 {
        let chars: Vec<char> = line.chars().collect();
        let mut depth = 0;
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            match *literal {
                Literal::String => {
                    match c {
                        '\\' => i += 1,
                        '"' => *literal = Literal::None,
                        _ => {}
                    }
                    i += 1;
                    continue;
                }
                Literal::Raw(hashes) => {
                    if c == '"'
                        && chars[i + 1..]
                            .iter()
                            .take(hashes)
                            .filter(|c| **c == '#')
                            .count()
                            == hashes
                    {
                        *literal = Literal::None;
                        i += hashes;
                    }
                    i += 1;
                    continue;
                }
                Literal::None => {}
            }
            let starts_word = i == 0 || !(chars[i - 1].is_alphanumeric() || chars[i - 1] == '_');
            match c {
                'r' if starts_word
                    || (i == 1 && chars[0] == 'b')
                    || (i >= 2
                        && chars[i - 1] == 'b'
                        && !(chars[i - 2].is_alphanumeric() || chars[i - 2] == '_')) =>
                {
                    let hashes = chars[i + 1..].iter().take_while(|c| **c == '#').count();
                    if chars.get(i + 1 + hashes) == Some(&'"') {
                        *literal = Literal::Raw(hashes);
                        i += hashes + 1;
                    }
                }
                '"' => *literal = Literal::String,
                '\'' => {
                    // A char literal such as '{' or '\''; a lifetime has no closing quote.
                    if chars.get(i + 1) == Some(&'\\') {
                        i += 3;
                    } else if chars.get(i + 2) == Some(&'\'') {
                        i += 2;
                    }
                }
                '/' if chars.get(i + 1) == Some(&'/') => break,
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
            i += 1;
        }
        depth
    }
}
