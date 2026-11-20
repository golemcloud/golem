use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

/// An SDK whose components Golem measures. The serialized names are part of the nightly series
/// identity, so renaming one starts a new series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "analysis", derive(clap::ValueEnum))]
#[cfg_attr(feature = "analysis", value(rename_all = "lower"))]
pub enum Sdk {
    Ts,
    Effect,
    Scala,
    Rust,
    Moonbit,
    Go,
}

impl Sdk {
    pub const ALL: [Sdk; 6] = [
        Sdk::Ts,
        Sdk::Effect,
        Sdk::Scala,
        Sdk::Rust,
        Sdk::Moonbit,
        Sdk::Go,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Sdk::Ts => "ts",
            Sdk::Effect => "effect",
            Sdk::Scala => "scala",
            Sdk::Rust => "rust",
            Sdk::Moonbit => "moonbit",
            Sdk::Go => "go",
        }
    }
}

impl Display for Sdk {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Sdk {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Sdk::ALL
            .into_iter()
            .find(|sdk| sdk.name() == s)
            .ok_or_else(|| format!("unknown SDK `{s}`"))
    }
}

/// The three size fixtures every SDK provides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FixtureKind {
    /// The agent `golem app new` generates from the SDK's default template.
    StarterAgent,
    /// The tool provider of the SDK's `tool-streaming*` test component.
    Tool,
    /// An agent with state, an HTTP mount, RPC, config and a tool call, a known 100 KB user-code
    /// constant and one known dependency.
    RepresentativeAgent,
}

impl FixtureKind {
    pub const ALL: [FixtureKind; 3] = [
        FixtureKind::StarterAgent,
        FixtureKind::Tool,
        FixtureKind::RepresentativeAgent,
    ];

    pub fn name(self) -> &'static str {
        match self {
            FixtureKind::StarterAgent => "starter-agent",
            FixtureKind::Tool => "tool",
            FixtureKind::RepresentativeAgent => "representative-agent",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Fixture {
    pub sdk: Sdk,
    pub kind: FixtureKind,
}

impl Fixture {
    /// The identity of the fixture's nightly size and cold-start series.
    pub fn series(&self) -> String {
        format!("{}/{}", self.sdk.name(), self.kind.name())
    }
}

/// Every size fixture, in SDK order, then fixture order.
pub fn fixtures() -> Vec<Fixture> {
    Sdk::ALL
        .into_iter()
        .flat_map(|sdk| FixtureKind::ALL.map(|kind| Fixture { sdk, kind }))
        .collect()
}

/// The checked call list of one fixture. The nightly cold-start job times the first call; the
/// size job and S1 run every call, and any failed call makes the measurement invalid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FixtureDescriptor {
    pub calls: Vec<CheckedCall>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckedCall {
    pub target: CallTarget,
    /// Fixed arguments, in the JSON form the CLI accepts for the call kind.
    pub arguments: serde_json::Value,
    /// The result the call must return.
    pub expected: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CallTarget {
    /// An agent method invocation. Each run uses a fresh agent name.
    Invoke { agent_type: String, method: String },
    /// An HTTP request to the agent's mount.
    Http { method: String, path: String },
    /// A tool invocation.
    Tool { tool: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn every_sdk_has_the_three_fixtures_under_stable_series_names() {
        let series: Vec<String> = fixtures().iter().map(Fixture::series).collect();
        let expected: Vec<String> = ["ts", "effect", "scala", "rust", "moonbit", "go"]
            .iter()
            .flat_map(|sdk| {
                ["starter-agent", "tool", "representative-agent"]
                    .iter()
                    .map(move |kind| format!("{sdk}/{kind}"))
            })
            .collect();
        assert_eq!(series, expected);
        for sdk in Sdk::ALL {
            assert_eq!(sdk.to_string().parse::<Sdk>(), Ok(sdk));
            assert_eq!(serde_json::to_value(sdk).unwrap(), sdk.name());
        }
    }

    #[test]
    fn descriptor_reads_each_call_kind_and_rejects_unknown_fields() {
        let descriptor: FixtureDescriptor = serde_json::from_str(
            r#"{"calls": [
                {"target": {"kind": "invoke", "agentType": "Counter", "method": "increment"},
                 "arguments": [], "expected": 1},
                {"target": {"kind": "http", "method": "GET", "path": "/counters/a"},
                 "arguments": null, "expected": {"value": 1}},
                {"target": {"kind": "tool", "tool": "echo"}, "arguments": ["hi"], "expected": "hi"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(
            descriptor
                .calls
                .iter()
                .map(|c| c.target.clone())
                .collect::<Vec<_>>(),
            vec![
                CallTarget::Invoke {
                    agent_type: "Counter".to_string(),
                    method: "increment".to_string()
                },
                CallTarget::Http {
                    method: "GET".to_string(),
                    path: "/counters/a".to_string()
                },
                CallTarget::Tool {
                    tool: "echo".to_string()
                },
            ]
        );
        assert!(
            serde_json::from_str::<FixtureDescriptor>(r#"{"calls": [], "timeout": 1}"#).is_err()
        );
        assert!(
            serde_json::from_str::<FixtureDescriptor>(
                r#"{"calls": [{"target": {"kind": "tool", "tool": "t", "retries": 2},
                    "arguments": null, "expected": null}]}"#
            )
            .is_err()
        );
    }
}
