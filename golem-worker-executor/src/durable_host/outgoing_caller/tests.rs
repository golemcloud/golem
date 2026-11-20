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

use super::{CallingCode, outgoing_caller};
use crate::test_callers::{TEST_ISSUER, agent_principal, sample_caller, user_principal};
use golem_common::model::AgentId;
use golem_common::model::caller::{Caller, Principal, ToolPrincipal};
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::tool::ToolName;
use test_r::test;
use uuid::Uuid;

fn agent(name: &str) -> Principal {
    agent_principal(AgentId {
        component_id: ComponentId(Uuid::from_u128(0xa0)),
        agent_id: format!("Worker(\"{name}\")"),
    })
}

fn tool(name: &str) -> ToolPrincipal {
    ToolPrincipal {
        issuer: TEST_ISSUER.to_string(),
        environment_id: EnvironmentId(Uuid::from_u128(0xe0)),
        tool_name: ToolName::try_from(name).unwrap(),
    }
}

/// `U` with the tokens of the sample caller in both roles: the caller at an entry point.
fn entry_by_user() -> Caller {
    Caller {
        original: user_principal(),
        immediate: user_principal(),
        tokens: sample_caller().tokens,
    }
}

fn with_immediate(immediate: Principal) -> Caller {
    Caller {
        immediate,
        ..entry_by_user()
    }
}

#[test]
fn outgoing_caller_table() {
    let a = agent("a");
    let t = Principal::Tool(tool("report"));
    let rows: Vec<(&str, Caller, CallingCode, Caller)> = vec![
        (
            "the agent called at an entry point by U",
            entry_by_user(),
            CallingCode::Agent(a.clone()),
            with_immediate(a.clone()),
        ),
        (
            "the agent called by another agent for U",
            with_immediate(agent("b")),
            CallingCode::Agent(a.clone()),
            with_immediate(a.clone()),
        ),
        (
            "a tool called by A for U",
            with_immediate(a.clone()),
            CallingCode::Tool(tool("report")),
            with_immediate(t.clone()),
        ),
        (
            "a middleware layer of a call to the tool, which A made for U",
            with_immediate(a.clone()),
            CallingCode::Tool(tool("report")),
            with_immediate(t.clone()),
        ),
        (
            "the shell called by A for U",
            with_immediate(a.clone()),
            CallingCode::Shell {
                host: Some(a.clone()),
            },
            with_immediate(a.clone()),
        ),
        (
            "the shell called by tool T, which A hosts",
            with_immediate(t.clone()),
            CallingCode::Shell {
                host: Some(a.clone()),
            },
            with_immediate(t.clone()),
        ),
        (
            "the shell started on A by U at an entry point",
            entry_by_user(),
            CallingCode::Shell {
                host: Some(a.clone()),
            },
            with_immediate(a.clone()),
        ),
        (
            "the shell started on A by an anonymous caller",
            Caller::anonymous(),
            CallingCode::Shell {
                host: Some(a.clone()),
            },
            Caller {
                original: Principal::Anonymous,
                immediate: a.clone(),
                tokens: Vec::new(),
            },
        ),
        (
            "a standalone shell started by U",
            entry_by_user(),
            CallingCode::Shell { host: None },
            entry_by_user(),
        ),
    ];
    for (row, invocation, code, expected) in rows {
        let actual = outgoing_caller(&invocation, &code);
        assert_eq!(actual, expected, "{row}");
        assert_eq!(actual.tokens, invocation.tokens, "{row}: tokens");
    }
}

/// The code that makes agent calls, creates, schedules, forks and tool calls builds no caller of
/// its own: each such caller comes from `DurableWorkerCtx::outgoing_caller`.
#[test]
fn outgoing_callers_come_from_one_function() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/durable_host");
    let mut found = Vec::new();
    let mut scanned = 0;
    for directory in ["rpc", "tool", "golem"] {
        let mut pending = vec![root.join(directory)];
        while let Some(path) = pending.pop() {
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name == "tests") {
                    continue;
                }
                for entry in std::fs::read_dir(&path).unwrap() {
                    pending.push(entry.unwrap().path());
                }
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if !name.ends_with(".rs") || name == "tests.rs" || name.ends_with("_tests.rs") {
                continue;
            }
            scanned += 1;
            let source = std::fs::read_to_string(&path).unwrap();
            let production = source
                .split("#[cfg(test)]\nmod tests {")
                .next()
                .unwrap_or_default();
            for (number, line) in production.lines().enumerate() {
                if ["Caller::entry(", "Caller::anonymous(", "Caller {"]
                    .iter()
                    .any(|builder| line.contains(builder))
                {
                    found.push(format!(
                        "{}:{}: {}",
                        path.strip_prefix(&root).unwrap().display(),
                        number + 1,
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(scanned > 20, "scanned only {scanned} files");
    assert!(
        found.is_empty(),
        "callers built outside outgoing_caller:\n{}",
        found.join("\n")
    );
}
