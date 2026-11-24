use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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

    pub fn presets(self) -> [&'static str; 2] {
        match self {
            Self::Ts | Self::Effect | Self::Scala => ["quick", "optimized"],
            Self::Rust | Self::Moonbit | Self::Go => ["debug", "release"],
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

    /// Relative to the `test-components/size-analysis` inputs directory.
    pub fn descriptor_path(&self) -> String {
        format!("sdk-fixtures/{}/{}.json", self.sdk, self.kind.name())
    }

    pub fn load_descriptor(&self, inputs: &std::path::Path) -> Result<FixtureDescriptor, String> {
        let path = inputs.join(self.descriptor_path());
        let bytes = std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let descriptor: FixtureDescriptor = serde_json::from_slice(&bytes)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        descriptor.bind("validate")?;
        Ok(descriptor)
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
    /// Constructor arguments may contain `${run}` in string values. One binding is shared by
    /// every call in a run, while a new run gets a fresh namespace.
    pub agents: BTreeMap<String, AgentBinding>,
    pub calls: Vec<CheckedCall>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentBinding {
    pub agent_type: String,
    pub arguments: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckedCall {
    pub target: CallTarget,
    /// Fixed arguments, in the JSON form the CLI accepts for the call kind.
    #[serde(deserialize_with = "serde_json::Value::deserialize")]
    pub arguments: serde_json::Value,
    /// The result the call must return.
    #[serde(deserialize_with = "serde_json::Value::deserialize")]
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
    /// An invocation of an agent binding in this descriptor.
    Invoke { agent: String, method: String },
    /// An HTTP request to the agent's mount.
    Http { method: String, path: String },
    /// A tool invocation.
    Tool { component: String, tool: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolArguments {
    pub argv: Vec<String>,
    /// The runner closes stdin after these bytes, including when the array is empty.
    pub stdin: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolExpected {
    /// A present JSON null is a checked unit result, not a missing result.
    #[serde(deserialize_with = "serde_json::Value::deserialize")]
    pub result: serde_json::Value,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpExpected {
    pub status: u16,
    #[serde(deserialize_with = "serde_json::Value::deserialize")]
    pub body: serde_json::Value,
}

impl AgentBinding {
    pub fn agent_id(&self) -> String {
        format!(
            "{}({})",
            self.agent_type,
            self.arguments
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        )
    }
}

impl FixtureDescriptor {
    /// Bind a complete call list to one URL-safe namespace. Validate before executing anything:
    /// a partial or empty descriptor must never produce a valid measurement.
    pub fn bind(&self, run: &str) -> Result<Self, String> {
        if run.is_empty()
            || !run
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            return Err("the fixture run name must be nonempty and URL-safe".into());
        }
        if self.calls.is_empty() {
            return Err("the fixture call list is empty".into());
        }
        let mut bound = self.clone();
        for (alias, binding) in &mut bound.agents {
            if alias.is_empty() || binding.agent_type.is_empty() {
                return Err("agent bindings need an alias and agent type".into());
            }
            for argument in &mut binding.arguments {
                if let serde_json::Value::String(value) = argument {
                    *value = value.replace("${run}", run);
                }
                if argument.to_string().contains("${") {
                    return Err(format!("unresolved token in agent binding {alias}"));
                }
            }
        }
        for call in &mut bound.calls {
            if call.arguments.to_string().contains("${") || call.expected.to_string().contains("${")
            {
                return Err(
                    "tokens are only allowed in agent binding strings and HTTP paths".into(),
                );
            }
            match &mut call.target {
                CallTarget::Invoke { agent, method } => {
                    if !bound.agents.contains_key(agent)
                        || method.is_empty()
                        || !call.arguments.is_array()
                    {
                        return Err(format!("invalid invocation of agent binding {agent}"));
                    }
                }
                CallTarget::Http { method, path } => {
                    *path = path.replace("${run}", run);
                    if method.is_empty() || !path.starts_with('/') || path.contains("${") {
                        return Err("HTTP calls need a method and resolved absolute path".into());
                    }
                    let expected: HttpExpected =
                        serde_json::from_value(call.expected.clone()).map_err(|e| e.to_string())?;
                    if !(100..=599).contains(&expected.status) {
                        return Err("invalid expected HTTP status".into());
                    }
                }
                CallTarget::Tool { component, tool } => {
                    if component.is_empty() || tool.is_empty() {
                        return Err("tool calls need an owner component and tool name".into());
                    }
                    serde_json::from_value::<ToolArguments>(call.arguments.clone())
                        .map_err(|e| e.to_string())?;
                    serde_json::from_value::<ToolExpected>(call.expected.clone())
                        .map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(bound)
    }
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
    fn every_fixture_has_a_valid_checked_call_list() {
        let inputs = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../test-components/size-analysis");
        for fixture in fixtures() {
            fixture
                .load_descriptor(&inputs)
                .unwrap_or_else(|error| panic!("{}: {error}", fixture.series()));
        }
        let payload = std::fs::read(inputs.join("sdk-fixtures/payload.txt")).unwrap();
        assert_eq!(payload.len(), 100_000);
        assert!(payload.iter().all(u8::is_ascii_hexdigit));
        assert_eq!(&payload[17..40], b"5f2fbb0f794e8f89231cefb");
        assert_eq!(&payload[99971..], b"421c4fcaa4d75f9680e254c78d98b");
    }

    #[test]
    fn starter_descriptors_share_one_identity_for_stateful_calls() {
        let inputs = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../test-components/size-analysis");
        for sdk in Sdk::ALL {
            let fixture = Fixture {
                sdk,
                kind: FixtureKind::StarterAgent,
            };
            let descriptor = fixture.load_descriptor(&inputs).unwrap();
            let bound = descriptor.bind("state-check").unwrap();
            let again = descriptor.bind("other-check").unwrap();
            assert_eq!(bound.agents.len(), 1);
            assert_ne!(bound.agents["main"], again.agents["main"]);
            let methods: Vec<_> = bound
                .calls
                .iter()
                .filter_map(|call| match &call.target {
                    CallTarget::Invoke { agent, method } => {
                        assert_eq!(agent, "main");
                        Some(method.as_str())
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(methods, ["value", "add", "add", "value"]);
            assert_eq!(bound.calls[1].arguments, serde_json::json!([7]));
            assert_eq!(bound.calls[2].arguments, serde_json::json!([11]));
        }
    }

    #[test]
    fn descriptor_reads_each_call_kind_and_rejects_unknown_fields() {
        let descriptor: FixtureDescriptor = serde_json::from_str(
            r#"{"agents": {"main": {"agentType": "Counter", "arguments": ["${run}"]}}, "calls": [
                {"target": {"kind": "invoke", "agent": "main", "method": "increment"},
                 "arguments": [], "expected": 1},
                {"target": {"kind": "http", "method": "GET", "path": "/counters/${run}"},
                 "arguments": null, "expected": {"status": 200, "body": 1}},
                {"target": {"kind": "tool", "component": "app:provider", "tool": "echo"},
                 "arguments": {"argv": ["hi"], "stdin": []},
                 "expected": {"result": "hi", "stdout": [], "stderr": []}}
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
                    agent: "main".to_string(),
                    method: "increment".to_string()
                },
                CallTarget::Http {
                    method: "GET".to_string(),
                    path: "/counters/${run}".to_string()
                },
                CallTarget::Tool {
                    component: "app:provider".to_string(),
                    tool: "echo".to_string()
                },
            ]
        );
        let first = descriptor.bind("first").unwrap();
        let second = descriptor.bind("second").unwrap();
        assert_eq!(first.agents["main"].agent_id(), "Counter(\"first\")");
        assert_eq!(second.agents["main"].agent_id(), "Counter(\"second\")");
        assert_eq!(descriptor.bind("first").unwrap(), first);
        assert_eq!(
            first.calls[1].target,
            CallTarget::Http {
                method: "GET".into(),
                path: "/counters/first".into()
            }
        );
        let mut invalid = descriptor.clone();
        invalid.agents.clear();
        assert!(invalid.bind("run").is_err());
        invalid.calls.clear();
        assert!(invalid.bind("run").is_err());
        assert!(descriptor.bind("not/a/path").is_err());
        for index in 0..descriptor.calls.len() {
            for token in ["${run}", "${unknown}"] {
                let mut invalid = descriptor.clone();
                invalid.calls[index].arguments = match index {
                    0 => serde_json::json!([{"nested": [token]}]),
                    1 => serde_json::json!({"nested": [token]}),
                    _ => serde_json::json!({"argv": [token], "stdin": []}),
                };
                assert!(invalid.bind("run").is_err());
                let mut invalid = descriptor.clone();
                invalid.calls[index].expected = match index {
                    0 => serde_json::json!({"nested": [token]}),
                    1 => serde_json::json!({"status": 200, "body": {"nested": [token]}}),
                    _ => {
                        serde_json::json!({"result": {"nested": [token]}, "stdout": [], "stderr": []})
                    }
                };
                assert!(invalid.bind("run").is_err());
            }
        }
        let mut missing_result = descriptor.clone();
        missing_result.calls[2]
            .expected
            .as_object_mut()
            .unwrap()
            .remove("result");
        assert!(missing_result.bind("run").is_err());
        assert!(serde_json::from_str::<ToolExpected>(r#"{"stdout":[],"stderr":[]}"#).is_err());
        let unit: ToolExpected =
            serde_json::from_str(r#"{"result":null,"stdout":[],"stderr":[]}"#).unwrap();
        assert_eq!(unit.result, serde_json::Value::Null);
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
