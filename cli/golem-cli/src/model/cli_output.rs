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

use crate::model::masking::MaskingConfig;
use anyhow::{anyhow, bail};
use serde::Serialize;
use serde::Serializer;
use serde_json::{Map, Value};
use std::collections::{BTreeSet, VecDeque};

pub const CLI_OUTPUT_TYPE_FIELD: &str = "$type";
const CLI_OUTPUT_TYPES_FIELD: &str = "x-golem-cli-output-types";
pub const COMMAND_OUTPUT_SCHEMA_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/command-output-schema/command-output.schema.json"
));

pub trait StructuredOutput: Serialize {
    const KIND: &'static str;

    fn type_name() -> String {
        Self::KIND.to_string()
    }

    fn serialize_masked<S>(self, serializer: S, config: MaskingConfig) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        Self: Sized,
    {
        let _ = config;
        self.serialize(serializer)
    }
}

pub fn command_output_schema_value() -> anyhow::Result<Value> {
    serde_json::from_str(COMMAND_OUTPUT_SCHEMA_JSON)
        .map_err(|err| anyhow!("Embedded command output schema must parse: {err}"))
}

pub fn command_output_type_names() -> anyhow::Result<Value> {
    let schema = command_output_schema_value()?;
    let entries = schema_output_type_entries(&schema)?;
    let names = entries
        .iter()
        .filter_map(|entry| entry.get("type"))
        .filter_map(Value::as_str)
        .map(|name| Value::String(name.to_string()))
        .collect::<Vec<_>>();
    Ok(Value::Array(names))
}

pub fn focused_command_output_schema(output_types: &[String]) -> anyhow::Result<Value> {
    if output_types.is_empty() {
        bail!("At least one output type must be specified");
    }

    let schema = command_output_schema_value()?;
    let definitions = schema
        .get("definitions")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("Command output schema is missing definitions"))?;
    let output_type_entries = schema_output_type_entries(&schema)?;
    let known_output_types = output_type_entries
        .iter()
        .filter_map(|entry| entry.get("type"))
        .filter_map(Value::as_str)
        .collect::<BTreeSet<_>>();

    let mut selected = BTreeSet::<String>::new();
    let mut reachable = BTreeSet::<String>::new();
    let mut queue = VecDeque::<String>::new();
    for output_type in output_types {
        if !known_output_types.contains(output_type.as_str()) {
            bail!(
                "Unknown output type: {output_type}; run `golem output-schema --types` to list known output types"
            );
        }
        if !definitions.contains_key(output_type) {
            bail!("Command output schema is missing definition {output_type}");
        }
        if selected.insert(output_type.clone()) && reachable.insert(output_type.clone()) {
            queue.push_back(output_type.clone());
        }
    }

    while let Some(name) = queue.pop_front() {
        let definition = definitions
            .get(&name)
            .ok_or_else(|| anyhow!("Command output schema is missing definition {name}"))?;
        let mut refs = BTreeSet::new();
        collect_definition_refs(definition, &mut refs);
        for reference in refs {
            if !definitions.contains_key(&reference) {
                bail!("Command output schema references missing definition {reference}");
            }
            if reachable.insert(reference.clone()) {
                queue.push_back(reference);
            }
        }
    }

    let mut focused = Map::new();
    if let Some(value) = schema.get("$schema") {
        focused.insert("$schema".to_string(), value.clone());
    }
    if let Some(value) = schema.get("title") {
        focused.insert("title".to_string(), value.clone());
    }
    focused.insert(
        "description".to_string(),
        Value::String(
            "Focused structured output schema for selected Golem CLI output types.".to_string(),
        ),
    );
    focused.insert(
        "oneOf".to_string(),
        Value::Array(
            selected
                .iter()
                .map(|output_type| json_ref(output_type))
                .collect(),
        ),
    );

    let mut pruned_definitions = Map::new();
    for name in &reachable {
        pruned_definitions.insert(
            name.clone(),
            definitions
                .get(name)
                .ok_or_else(|| anyhow!("Command output schema is missing definition {name}"))?
                .clone(),
        );
    }
    focused.insert("definitions".to_string(), Value::Object(pruned_definitions));

    focused.insert(
        CLI_OUTPUT_TYPES_FIELD.to_string(),
        Value::Array(
            output_type_entries
                .iter()
                .filter(|entry| {
                    entry
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|output_type| selected.contains(output_type))
                })
                .cloned()
                .collect(),
        ),
    );

    Ok(Value::Object(focused))
}

fn schema_output_type_entries(schema: &Value) -> anyhow::Result<&Vec<Value>> {
    schema
        .get(CLI_OUTPUT_TYPES_FIELD)
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Command output schema is missing {CLI_OUTPUT_TYPES_FIELD}"))
}

fn collect_definition_refs(value: &Value, refs: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(Value::as_str)
                && let Some(name) = reference.strip_prefix("#/definitions/")
            {
                refs.insert(name.to_string());
            }
            for value in object.values() {
                collect_definition_refs(value, refs);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_definition_refs(value, refs);
            }
        }
        _ => {}
    }
}

fn json_ref(definition_name: &str) -> Value {
    let mut reference = Map::new();
    reference.insert(
        "$ref".to_string(),
        Value::String(format!("#/definitions/{definition_name}")),
    );
    Value::Object(reference)
}

pub fn to_structured_output_value<Output: StructuredOutput>(
    output: Output,
) -> anyhow::Result<Value> {
    to_structured_output_value_masked(output, MaskingConfig::hide_secrets())
}

pub fn to_structured_output_value_masked<Output: StructuredOutput>(
    output: Output,
    config: MaskingConfig,
) -> anyhow::Result<Value> {
    let value = output.serialize_masked(serde_json::value::Serializer, config)?;
    let type_value = Value::String(Output::type_name());

    match value {
        Value::Object(fields) => Ok(Value::Object(with_structured_output_type::<Output>(
            fields, type_value,
        )?)),
        value => {
            let mut fields = Map::new();
            fields.insert(CLI_OUTPUT_TYPE_FIELD.to_string(), type_value);
            fields.insert("value".to_string(), value);
            Ok(Value::Object(fields))
        }
    }
}

fn with_structured_output_type<Output: StructuredOutput>(
    fields: Map<String, Value>,
    type_value: Value,
) -> anyhow::Result<Map<String, Value>> {
    let mut result = Map::new();
    result.insert(CLI_OUTPUT_TYPE_FIELD.to_string(), type_value);

    for (key, value) in fields {
        if key == CLI_OUTPUT_TYPE_FIELD {
            bail!(
                "CLI output model {} must not define reserved field {CLI_OUTPUT_TYPE_FIELD}",
                Output::KIND,
            );
        }
        result.insert(key, value);
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    #![allow(dead_code)]

    use crate::model::cli_output::{
        CLI_OUTPUT_TYPE_FIELD, StructuredOutput, command_output_type_names,
        focused_command_output_schema, to_structured_output_value,
        to_structured_output_value_masked,
    };
    use crate::model::masking::MaskingConfig;
    use crate::model::text::diff::DeployPlanView;
    use proptest::prelude::*;
    use quote::ToTokens;
    use serde_json::{Value, json};
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};
    use syn::{Expr, ImplItem, Item, ItemImpl, Lit, Type};
    use test_r::test;
    use walkdir::WalkDir;

    type OutputDocumentStrategy = BoxedStrategy<Value>;

    struct StructuredOutputTestEntry {
        rust_type: &'static str,
        output_type: &'static str,
        examples: fn() -> Vec<Value>,
        arbitrary: fn() -> OutputDocumentStrategy,
    }

    macro_rules! registry_entry {
        ($rust_type:literal, $output_type:literal, $arbitrary:expr) => {
            StructuredOutputTestEntry {
                rust_type: $rust_type,
                output_type: $output_type,
                examples: || {
                    let mut runner = proptest::test_runner::TestRunner::deterministic();
                    vec![
                        ($arbitrary)()
                            .new_tree(&mut runner)
                            .expect("example strategy should produce a value")
                            .current(),
                    ]
                },
                arbitrary: $arbitrary,
            }
        };
    }

    static STRUCTURED_OUTPUT_TEST_REGISTRY: &[StructuredOutputTestEntry] = &[
        registry_entry!(
            "AccountDeleteResult",
            "account.delete",
            arb_account_delete_result
        ),
        registry_entry!("AccountGetView", "account.get", arb_account_get_result),
        registry_entry!("AccountNewView", "account.new", arb_account_new_result),
        registry_entry!(
            "PermissionShareDeleteResult",
            "account.permission-share.delete",
            arb_permission_share_delete_result
        ),
        registry_entry!(
            "PermissionShareGetView",
            "account.permission-share.get",
            arb_permission_share_get_result
        ),
        registry_entry!(
            "PermissionShareListView",
            "account.permission-share.list",
            arb_permission_share_list_result
        ),
        registry_entry!(
            "PermissionShareNewView",
            "account.permission-share.new",
            arb_permission_share_new_result
        ),
        registry_entry!(
            "PermissionShareUpdateView",
            "account.permission-share.update",
            arb_permission_share_update_result
        ),
        registry_entry!(
            "AccountUpdateView",
            "account.update",
            arb_account_update_result
        ),
        registry_entry!("AgentTypeView", "agent-type.get", arb_agent_type_get_result),
        registry_entry!(
            "AgentTypeListView",
            "agent-type.list",
            arb_agent_type_list_result
        ),
        registry_entry!(
            "AgentCancelInvocationResult",
            "agent.cancel-invocation",
            arb_agent_cancel_invocation_result
        ),
        registry_entry!("AgentDeleteResult", "agent.delete", arb_agent_delete_result),
        registry_entry!(
            "AgentFileContentsResult",
            "agent.file-contents",
            arb_agent_file_contents_result
        ),
        registry_entry!("WorkerFilesView", "agent.files", arb_agent_files_result),
        registry_entry!("WorkerGetView", "agent.get", arb_agent_get_result),
        registry_entry!(
            "AgentInterruptResult",
            "agent.interrupt",
            arb_agent_interrupt_result
        ),
        registry_entry!("InvokeResultView", "agent.invoke", arb_agent_invoke_result),
        registry_entry!(
            "AgentsMetadataResponseView",
            "agent.list",
            arb_agent_list_result
        ),
        registry_entry!("WorkerCreateView", "agent.new", arb_agent_new_result),
        registry_entry!("AgentOplogEntryView", "agent.oplog", arb_agent_oplog_result),
        registry_entry!(
            "AgentPluginToggleResult",
            "agent.plugin-toggle",
            arb_agent_plugin_toggle_result
        ),
        registry_entry!(
            "AgentRedeployResult",
            "agent.redeploy",
            arb_agent_redeploy_result
        ),
        registry_entry!("AgentResumeResult", "agent.resume", arb_agent_resume_result),
        registry_entry!("AgentRevertResult", "agent.revert", arb_agent_revert_result),
        registry_entry!(
            "AgentSimulateCrashResult",
            "agent.simulate-crash",
            arb_agent_simulate_crash_result
        ),
        registry_entry!("AgentStreamEvent", "agent.stream", arb_agent_stream_event),
        registry_entry!(
            "TryUpdateAllWorkersResult",
            "agent.update",
            arb_agent_update_result
        ),
        registry_entry!(
            "TokenDeleteResult",
            "api-token.delete",
            arb_token_delete_result
        ),
        registry_entry!("TokenListView", "api-token.list", arb_token_list_result),
        registry_entry!("TokenNewView", "api-token.new", arb_token_new_result),
        registry_entry!(
            "HttpApiDeploymentGetView",
            "api.deployment.get",
            arb_api_deployment_get_result
        ),
        registry_entry!(
            "HttpApiDeploymentListView",
            "api.deployment.list",
            arb_api_deployment_list_result
        ),
        registry_entry!(
            "DomainRegistrationDeleteResult",
            "api.domain.delete",
            arb_api_domain_delete_result
        ),
        registry_entry!(
            "HttpApiDomainListView",
            "api.domain.list",
            arb_api_domain_list_result
        ),
        registry_entry!(
            "DomainRegistrationNewView",
            "api.domain.register",
            arb_api_domain_register_result
        ),
        registry_entry!(
            "HttpSecuritySchemeCreateView",
            "api.security-scheme.create",
            arb_api_security_scheme_create_result
        ),
        registry_entry!(
            "HttpSecuritySchemeDeleteView",
            "api.security-scheme.delete",
            arb_api_security_scheme_delete_result
        ),
        registry_entry!(
            "HttpSecuritySchemeGetView",
            "api.security-scheme.get",
            arb_api_security_scheme_get_result
        ),
        registry_entry!(
            "HttpSecuritySchemeListView",
            "api.security-scheme.list",
            arb_api_security_scheme_list_result
        ),
        registry_entry!(
            "HttpSecuritySchemeUpdateView",
            "api.security-scheme.update",
            arb_api_security_scheme_update_result
        ),
        registry_entry!("BuildResult", "build", arb_build_result),
        registry_entry!("CleanResult", "clean", arb_clean_result),
        registry_entry!("DeployPlanView", "deploy.plan", arb_deploy_plan_result),
        registry_entry!("DeployResultView", "deploy", arb_deploy_result),
        registry_entry!(
            "GenerateBridgeResult",
            "generate-bridge",
            arb_generate_bridge_result
        ),
        registry_entry!("NewAppResult", "new", arb_new_app_result),
        registry_entry!("TemplateListView", "templates", arb_template_list_result),
        registry_entry!(
            "ComponentGetView",
            "component.get",
            arb_component_get_result
        ),
        registry_entry!(
            "ComponentListView",
            "component.list",
            arb_component_list_result
        ),
        registry_entry!(
            "ComponentManifestTraceView",
            "component.manifest-trace",
            arb_component_manifest_trace_result
        ),
        registry_entry!(
            "DeploymentNewView",
            "deploy.deployment",
            arb_deployment_create_result
        ),
        registry_entry!("DeploymentDiff", "deploy.diff", arb_deployment_diff_result),
        registry_entry!(
            "DeploymentListView",
            "deploy.deployments",
            arb_deployment_list_result
        ),
        registry_entry!(
            "EnvironmentListView",
            "environment.list",
            arb_environment_list_result
        ),
        registry_entry!(
            "EnvironmentSyncDeploymentOptionsResult",
            "environment.sync-deployment-options",
            arb_environment_sync_deployment_options_result
        ),
        registry_entry!(
            "EnvironmentSetupPlanView",
            "deploy.environment-setup-plan",
            arb_environment_setup_plan_result
        ),
        registry_entry!(
            "PluginRegistrationGetView",
            "plugin.get",
            arb_plugin_get_result
        ),
        registry_entry!("PluginListView", "plugin.list", arb_plugin_list_result),
        registry_entry!(
            "PluginRegistrationRegisterView",
            "plugin.register",
            arb_plugin_register_result
        ),
        registry_entry!(
            "PluginUnregisterResult",
            "plugin.unregister",
            arb_plugin_unregister_result
        ),
        registry_entry!(
            "ProfileConfigSetFormatResult",
            "profile.config.set-format",
            arb_profile_config_set_format_result
        ),
        registry_entry!(
            "ProfileDeleteResult",
            "profile.delete",
            arb_profile_delete_result
        ),
        registry_entry!("ProfileView", "profile.get", arb_profile_get_result),
        registry_entry!("ProfileListView", "profile.list", arb_profile_list_result),
        registry_entry!(
            "ProfileCreateResult",
            "profile.new",
            arb_profile_create_result
        ),
        registry_entry!(
            "ProfileSwitchResult",
            "profile.switch",
            arb_profile_switch_result
        ),
        registry_entry!(
            "ResourceDefinitionCreateView",
            "resource.create",
            arb_resource_create_result
        ),
        registry_entry!(
            "ResourceDefinitionDeleteView",
            "resource.delete",
            arb_resource_delete_result
        ),
        registry_entry!(
            "ResourceDefinitionGetView",
            "resource.get",
            arb_resource_get_result
        ),
        registry_entry!(
            "ResourceDefinitionListView",
            "resource.list",
            arb_resource_list_result
        ),
        registry_entry!(
            "ResourceDefinitionUpdateView",
            "resource.update",
            arb_resource_update_result
        ),
        registry_entry!(
            "RetryPolicyCreateView",
            "retry-policy.create",
            arb_retry_policy_create_result
        ),
        registry_entry!(
            "RetryPolicyDeleteView",
            "retry-policy.delete",
            arb_retry_policy_delete_result
        ),
        registry_entry!(
            "RetryPolicyGetView",
            "retry-policy.get",
            arb_retry_policy_get_result
        ),
        registry_entry!(
            "RetryPolicyListView",
            "retry-policy.list",
            arb_retry_policy_list_result
        ),
        registry_entry!(
            "RetryPolicyUpdateView",
            "retry-policy.update",
            arb_retry_policy_update_result
        ),
        registry_entry!(
            "SecretCreateView",
            "secret.create",
            arb_secret_create_result
        ),
        registry_entry!(
            "SecretDeleteView",
            "secret.delete",
            arb_secret_delete_result
        ),
        registry_entry!("SecretGetView", "secret.get", arb_secret_get_result),
        registry_entry!("SecretListView", "secret.list", arb_secret_list_result),
        registry_entry!(
            "SecretUpdateView",
            "secret.update-value",
            arb_secret_update_value_result
        ),
    ];

    #[derive(Debug, Clone)]
    struct OutputImpl {
        rust_type: String,
        kind: String,
        file: PathBuf,
        tuple_field_type: Option<String>,
    }

    impl OutputImpl {
        fn type_name(&self) -> String {
            self.kind.clone()
        }
    }

    #[derive(Default)]
    struct SourceSummary {
        outputs: Vec<OutputImpl>,
        tuple_field_types_by_struct: BTreeMap<String, String>,
    }

    #[test]
    fn cli_output_schema_source_kinds_are_consistent() {
        let summary = collect_source_summary();
        let mut errors = Vec::new();

        if let Ok(path) = std::env::var("GOLEM_CLI_OUTPUT_SUMMARY_MD") {
            if let Some(parent) = Path::new(&path).parent() {
                std::fs::create_dir_all(parent)
                    .unwrap_or_else(|err| panic!("failed to create {}: {err}", parent.display()));
            }
            std::fs::write(&path, render_markdown_summary(&summary.outputs))
                .unwrap_or_else(|err| panic!("failed to write {path}: {err}"));
        }

        let mut by_type_name = BTreeMap::<String, Vec<&OutputImpl>>::new();
        for output in &summary.outputs {
            by_type_name
                .entry(output.type_name())
                .or_default()
                .push(output);
        }

        for (type_name, outputs) in by_type_name {
            if outputs.len() > 1 {
                errors.push(format!(
                    "duplicate CLI output $type {type_name}: {}",
                    outputs
                        .iter()
                        .map(|output| output.rust_type.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }

        for output in &summary.outputs {
            if !is_valid_kind(&output.kind) {
                errors.push(format!(
                    "{} has invalid KIND {:?}",
                    output.rust_type, output.kind
                ));
            }

            if let Some(tuple_field_type) = &output.tuple_field_type
                && is_known_non_object_type(tuple_field_type)
            {
                errors.push(format!(
                        "{} is a StructuredOutput tuple wrapper around non-object type `{}`; use a named output struct instead",
                        output.rust_type, tuple_field_type,
                    ));
            }
        }

        assert!(errors.is_empty(), "\n{}", errors.join("\n"));
    }

    #[test]
    fn cli_output_schema_matches_source_registry() {
        let source_entries = source_output_entries();
        let schema = load_command_output_schema();
        let schema_entries = schema_output_entries(&schema);
        let registry_entries = registry_output_entries();

        assert_eq!(registry_entries, source_entries);
        assert_eq!(schema_entries, source_entries);

        let definitions = schema
            .get("definitions")
            .and_then(Value::as_object)
            .expect("schema must have object definitions");
        let one_of_refs = schema
            .get("oneOf")
            .and_then(Value::as_array)
            .expect("schema must have array oneOf")
            .iter()
            .map(|entry| {
                entry
                    .get("$ref")
                    .and_then(Value::as_str)
                    .expect("oneOf entry must have string $ref")
                    .strip_prefix("#/definitions/")
                    .expect("oneOf $ref must point to #/definitions")
                    .to_string()
            })
            .collect::<BTreeSet<_>>();

        let schema_types = schema_entries.keys().cloned().collect::<BTreeSet<_>>();
        let definition_types = definitions.keys().cloned().collect::<BTreeSet<_>>();

        let missing_definitions = schema_types
            .difference(&definition_types)
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            missing_definitions.is_empty(),
            "each output type must have a schema definition, missing: {missing_definitions:?}"
        );
        assert_eq!(
            one_of_refs, schema_types,
            "oneOf refs must match output types"
        );

        jsonschema::options()
            .build(&schema)
            .expect("command output schema must be a valid JSON schema");
    }

    #[test]
    fn cli_output_schema_types_lists_only_type_names() {
        let types = command_output_type_names().expect("type names should render");
        let types = types.as_array().expect("types output must be an array");

        assert!(
            types.iter().all(Value::is_string),
            "types output must contain only strings"
        );
        assert!(types.iter().any(|value| value == "agent.oplog"));
        assert!(types.iter().any(|value| value == "agent.stream"));
    }

    #[test]
    fn cli_output_schema_output_definitions_have_agent_metadata() {
        let schema = load_command_output_schema();
        let definitions = schema_definitions(&schema);

        for output_type in schema_output_entries(&schema).keys() {
            let definition = definitions
                .get(output_type)
                .unwrap_or_else(|| panic!("missing definition for {output_type}"));
            for field in ["description", "x-golem-output-mode", "x-golem-command"] {
                assert!(
                    definition.get(field).is_some(),
                    "{output_type} must define top-level {field} metadata"
                );
            }

            let output_mode = definition
                .get("x-golem-output-mode")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("{output_type} must define string x-golem-output-mode"));
            assert!(
                matches!(output_mode, "single" | "stream" | "multi-document"),
                "{output_type} has invalid x-golem-output-mode {output_mode:?}"
            );

            let primary_command = definition
                .get("x-golem-command")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("{output_type} must define string x-golem-command"));
            assert!(
                !primary_command.trim().is_empty(),
                "{output_type} must define non-empty x-golem-command"
            );

            if let Some(commands) = definition.get("x-golem-commands") {
                let commands = commands
                    .as_array()
                    .unwrap_or_else(|| panic!("{output_type} x-golem-commands must be an array"));
                assert!(
                    !commands.is_empty(),
                    "{output_type} x-golem-commands must not be empty"
                );
                assert!(
                    commands.iter().all(|command| command
                        .as_str()
                        .is_some_and(|command| !command.trim().is_empty())),
                    "{output_type} x-golem-commands must contain only non-empty strings"
                );
                assert!(
                    commands.iter().any(|command| command == primary_command),
                    "{output_type} x-golem-commands must include x-golem-command"
                );
            }
        }
    }

    #[test]
    fn cli_output_schema_focus_prunes_unrelated_definitions() {
        let schema = focused_command_output_schema(&["agent.oplog".to_string()])
            .expect("focused schema should render");
        let definitions = schema_definitions(&schema);

        assert!(definitions.contains_key("agent.oplog"));
        assert!(definitions.contains_key("PublicOplogEntry"));
        assert!(!definitions.contains_key("agent.list"));
        assert!(!definitions.contains_key("component.list"));

        let entries = schema_output_entries(&schema);
        assert_eq!(
            entries.keys().collect::<Vec<_>>(),
            vec![&"agent.oplog".to_string()]
        );

        let validator = jsonschema::options()
            .build(&schema)
            .expect("focused command output schema must be valid JSON schema");
        let example = (arb_agent_oplog_result())
            .new_tree(&mut proptest::test_runner::TestRunner::deterministic())
            .expect("oplog strategy should produce value")
            .current();
        assert!(
            validator.is_valid(&example),
            "focused schema should accept agent.oplog example: {:?}",
            validator
                .iter_errors(&example)
                .map(|error| error.to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn cli_output_schema_focus_supports_multiple_types() {
        let schema =
            focused_command_output_schema(&["agent.oplog".to_string(), "agent.stream".to_string()])
                .expect("focused schema should render");
        let definitions = schema_definitions(&schema);

        assert!(definitions.contains_key("agent.oplog"));
        assert!(definitions.contains_key("agent.stream"));
        assert!(!definitions.contains_key("agent.list"));

        let entries = schema_output_entries(&schema);
        assert_eq!(
            entries.keys().cloned().collect::<BTreeSet<_>>(),
            BTreeSet::from_iter(["agent.oplog".to_string(), "agent.stream".to_string()])
        );
    }

    #[test]
    fn cli_output_schema_focus_deduplicates_requested_types() {
        let schema =
            focused_command_output_schema(&["agent.oplog".to_string(), "agent.oplog".to_string()])
                .expect("focused schema should render");

        let one_of = schema
            .get("oneOf")
            .and_then(Value::as_array)
            .expect("focused schema must have oneOf");
        assert_eq!(one_of.len(), 1);

        let validator = jsonschema::options()
            .build(&schema)
            .expect("focused command output schema must be valid JSON schema");
        let example = (arb_agent_oplog_result())
            .new_tree(&mut proptest::test_runner::TestRunner::deterministic())
            .expect("oplog strategy should produce value")
            .current();
        assert!(validator.is_valid(&example));
    }

    #[test]
    fn cli_output_schema_focus_rejects_helper_definition_names() {
        let error = focused_command_output_schema(&["JsonValue".to_string()])
            .expect_err("helper definition should not be accepted as an output type");

        assert!(error.to_string().contains("Unknown output type: JsonValue"));
    }

    #[test]
    fn cli_output_schema_focus_rejects_unknown_type() {
        let error = focused_command_output_schema(&["unknown".to_string()])
            .expect_err("unknown output type should fail");

        assert!(error.to_string().contains("Unknown output type: unknown"));
    }

    #[test]
    fn agent_list_structured_output_masks_secret_config_paths() {
        let output = crate::model::worker::AgentsMetadataResponseView {
            agents: vec![crate::model::worker::AgentMetadataView {
                component_name: golem_common::model::component::ComponentName(
                    "component".to_string(),
                ),
                agent_name: crate::model::worker::RawAgentId("agent()".to_string()),
                created_by: golem_common::model::account::AccountId(uuid::Uuid::nil()),
                environment_id: golem_common::model::environment::EnvironmentId(uuid::Uuid::nil()),
                env: BTreeMap::new().into_iter().collect(),
                default_env: BTreeMap::new().into_iter().collect(),
                config: vec![golem_common::model::worker::AgentConfigEntryDto {
                    path: vec!["db".to_string(), "password".to_string()],
                    value: golem_common::base_model::json::NormalizedJsonValue(json!(
                        "runtime-secret"
                    )),
                }],
                default_config: vec![golem_common::model::worker::AgentConfigEntryDto {
                    path: vec!["db".to_string(), "password".to_string()],
                    value: golem_common::base_model::json::NormalizedJsonValue(json!(
                        "default-secret"
                    )),
                }],
                status: golem_common::model::AgentStatus::Idle,
                component_revision: golem_common::model::component::ComponentRevision::new(1)
                    .unwrap(),
                retry_count: 0,
                pending_invocation_count: 0,
                updates: vec![],
                created_at: "2024-01-01T00:00:00Z".parse().unwrap(),
                last_error: None,
                component_size: 0,
                total_linear_memory_size: 0,
                exported_resource_instances: BTreeMap::new().into_iter().collect(),
                source_language: crate::agent_id_display::SourceLanguage::default(),
                secret_config_paths: BTreeSet::from_iter(["db.password".to_string()]),
            }],
            cursors: BTreeMap::new(),
        };

        let value = to_structured_output_value_masked(output, MaskingConfig::hide_secrets())
            .expect("agent list should serialize");

        assert_eq!(value["agents"][0]["config"][0]["value"], json!("***"));
        assert_eq!(
            value["agents"][0]["defaultConfig"][0]["value"],
            json!("***")
        );
        assert!(!value.to_string().contains("runtime-secret"));
        assert!(!value.to_string().contains("default-secret"));
    }

    #[test]
    fn cli_output_schema_validates_schema_native_secret_outputs() {
        let schema = load_command_output_schema();
        let validator = jsonschema::options()
            .build(&schema)
            .expect("command output schema must be a valid JSON schema");

        let secret = golem_client::model::AgentSecretDto {
            id: golem_common::model::agent_secret::AgentSecretId(uuid::Uuid::nil()),
            environment_id: golem_common::model::environment::EnvironmentId(uuid::Uuid::nil()),
            path: golem_common::model::agent_secret::CanonicalAgentSecretPath(vec![
                "token".to_string(),
            ]),
            revision: golem_common::model::agent_secret::AgentSecretRevision::new(1)
                .expect("static secret revision should be valid"),
            secret_type: golem_common::schema::SchemaGraph::anonymous(
                golem_common::schema::SchemaType::string(),
            ),
            secret_value: Some(golem_common::schema::SchemaValue::String(
                "super-secret".to_string(),
            )),
        };

        let outputs = vec![
            to_structured_output_value_masked(
                crate::model::text::secret::SecretCreateView(secret.clone().into()),
                MaskingConfig::hide_secrets(),
            )
            .expect("secret.create should serialize"),
            to_structured_output_value_masked(
                crate::model::text::secret::SecretDeleteView(secret.clone().into()),
                MaskingConfig::hide_secrets(),
            )
            .expect("secret.delete should serialize"),
            to_structured_output_value_masked(
                crate::model::text::secret::SecretGetView(secret.clone().into()),
                MaskingConfig::hide_secrets(),
            )
            .expect("secret.get should serialize"),
            to_structured_output_value_masked(
                crate::model::text::secret::SecretUpdateView(secret.clone().into()),
                MaskingConfig::hide_secrets(),
            )
            .expect("secret.update-value should serialize"),
            to_structured_output_value_masked(
                crate::model::text::secret::SecretListView {
                    secrets: vec![secret.into()],
                    environment_name: "generated-environment".to_string(),
                    show_ids: false,
                },
                MaskingConfig::hide_secrets(),
            )
            .expect("secret.list should serialize"),
        ];

        for output in outputs {
            assert!(
                validator.is_valid(&output),
                "schema should accept schema-native secret output: {:?}",
                validator
                    .iter_errors(&output)
                    .map(|error| error.to_string())
                    .collect::<Vec<_>>()
            );

            let secret_view = output
                .get("secrets")
                .and_then(Value::as_array)
                .and_then(|secrets| secrets.first())
                .unwrap_or(&output);

            assert_eq!(secret_view["secretType"]["root"]["kind"], json!("string"));
            assert_eq!(secret_view["secretValue"]["kind"], json!("string"));
            assert_eq!(secret_view["secretValue"]["value"], json!("***"));
        }
    }

    #[test]
    fn cli_output_schema_validates_schema_native_component_and_agent_outputs() {
        let schema = load_command_output_schema();
        let validator = jsonschema::options()
            .build(&schema)
            .expect("command output schema must be a valid JSON schema");
        let mut runner = proptest::test_runner::TestRunner::deterministic();

        let component = arb_component_view()
            .new_tree(&mut runner)
            .expect("component strategy should produce a value")
            .current();
        let agent_type = arb_deployed_registered_agent_type()
            .new_tree(&mut runner)
            .expect("agent type strategy should produce a value")
            .current();

        let outputs = vec![
            to_structured_output_value(crate::model::text::component::ComponentGetView(
                component.clone(),
            ))
            .expect("component.get should serialize"),
            to_structured_output_value(crate::model::text::component::ComponentListView {
                components: vec![component],
            })
            .expect("component.list should serialize"),
            to_structured_output_value(crate::model::text::agent::AgentTypeListView {
                agent_types: vec![agent_type],
            })
            .expect("agent-type.list should serialize"),
            to_structured_output_value(crate::model::text::worker::AgentOplogEntryView {
                index: 0,
                entry: sample_public_oplog_entries()
                    .into_iter()
                    .next()
                    .expect("sample oplog entries should not be empty"),
            })
            .expect("agent.oplog should serialize"),
        ];

        for output in outputs {
            assert!(
                validator.is_valid(&output),
                "schema should accept schema-native output: {:?}",
                validator
                    .iter_errors(&output)
                    .map(|error| error.to_string())
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn cli_output_schema_validates_discriminated_documents() {
        let schema = load_command_output_schema();
        let validator = jsonschema::options()
            .build(&schema)
            .expect("command output schema must be a valid JSON schema");

        let definitions = schema_definitions(&schema);
        for output_type in schema_output_entries(&schema).keys() {
            if !is_discriminator_only_definition(
                definitions
                    .get(output_type)
                    .unwrap_or_else(|| panic!("missing definition for {output_type}")),
            ) {
                continue;
            }

            let value = json!({ CLI_OUTPUT_TYPE_FIELD: output_type });
            assert!(
                validator.is_valid(&value),
                "schema should accept minimal document for {output_type}: {:?}",
                validator
                    .iter_errors(&value)
                    .map(|error| error.to_string())
                    .collect::<Vec<_>>()
            );
        }

        let missing_type = json!({ "ok": true });
        assert!(
            !validator.is_valid(&missing_type),
            "schema must reject output documents without {CLI_OUTPUT_TYPE_FIELD}"
        );

        let unknown_type = json!({ CLI_OUTPUT_TYPE_FIELD: "unknown" });
        assert!(
            !validator.is_valid(&unknown_type),
            "schema must reject unknown output document types"
        );
    }

    #[test]
    fn cli_output_schema_exact_registered_schemas_reject_extra_fields() {
        let schema = load_command_output_schema();
        let definitions = schema_definitions(&schema);
        let validator = jsonschema::options()
            .build(&schema)
            .expect("command output schema must be a valid JSON schema");

        for entry in STRUCTURED_OUTPUT_TEST_REGISTRY.iter().filter(|entry| {
            !definition_allows_additional_properties(
                definitions
                    .get(entry.output_type)
                    .unwrap_or_else(|| panic!("missing definition for {}", entry.output_type)),
            )
        }) {
            for mut example in (entry.examples)() {
                let Some(object) = example.as_object_mut() else {
                    panic!("example for {} must be an object", entry.output_type);
                };
                object.insert("unexpectedExtraField".to_string(), json!(true));

                assert!(
                    !validator.is_valid(&example),
                    "exact schema should reject extra fields for {}",
                    entry.output_type,
                );
            }
        }
    }

    #[test]
    fn cli_output_schema_validates_registered_examples() {
        let schema = load_command_output_schema();
        let validator = jsonschema::options()
            .build(&schema)
            .expect("command output schema must be a valid JSON schema");

        for entry in STRUCTURED_OUTPUT_TEST_REGISTRY {
            for example in (entry.examples)() {
                assert!(
                    validator.is_valid(&example),
                    "schema should accept example for {}: {:?}",
                    entry.output_type,
                    validator
                        .iter_errors(&example)
                        .map(|error| error.to_string())
                        .collect::<Vec<_>>()
                );
            }
        }
    }

    proptest! {
        #[test]
        fn cli_output_schema_accepts_registered_generated_examples(value in arb_registered_output_document()) {
            let schema = load_command_output_schema();
            let validator = jsonschema::options()
                .build(&schema)
                .expect("command output schema must be a valid JSON schema");

            prop_assert!(
                validator.is_valid(&value),
                "schema should accept generated example: {:?}",
                validator
                    .iter_errors(&value)
                    .map(|error| error.to_string())
                    .collect::<Vec<_>>()
            );
        }
    }

    fn load_command_output_schema() -> Value {
        serde_json::from_str(crate::model::cli_output::COMMAND_OUTPUT_SCHEMA_JSON)
            .expect("embedded command output schema must parse")
    }

    fn schema_output_entries(schema: &Value) -> BTreeMap<String, String> {
        schema
            .get("x-golem-cli-output-types")
            .and_then(Value::as_array)
            .expect("schema must have array x-golem-cli-output-types")
            .iter()
            .map(|entry| {
                let output_type = entry
                    .get("type")
                    .and_then(Value::as_str)
                    .expect("schema output entry must have string type")
                    .to_string();
                let rust_type = entry
                    .get("rustType")
                    .and_then(Value::as_str)
                    .expect("schema output entry must have string rustType")
                    .to_string();
                (output_type, rust_type)
            })
            .collect()
    }

    fn schema_definitions(schema: &Value) -> &serde_json::Map<String, Value> {
        schema
            .get("definitions")
            .and_then(Value::as_object)
            .expect("schema must have object definitions")
    }

    fn is_discriminator_only_definition(definition: &Value) -> bool {
        definition
            .get("required")
            .and_then(Value::as_array)
            .is_some_and(|required| {
                required.len() == 1
                    && required
                        .first()
                        .and_then(Value::as_str)
                        .is_some_and(|field| field == CLI_OUTPUT_TYPE_FIELD)
            })
    }

    fn definition_allows_additional_properties(definition: &Value) -> bool {
        definition
            .get("additionalProperties")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }

    fn registry_output_entries() -> BTreeMap<String, String> {
        STRUCTURED_OUTPUT_TEST_REGISTRY
            .iter()
            .map(|entry| (entry.output_type.to_string(), entry.rust_type.to_string()))
            .collect()
    }

    fn source_output_entries() -> BTreeMap<String, String> {
        collect_source_summary()
            .outputs
            .into_iter()
            .map(|output| (output.type_name(), output.rust_type))
            .collect()
    }

    fn collect_source_summary() -> SourceSummary {
        let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut summary = SourceSummary::default();

        for entry in WalkDir::new(&source_root)
            .into_iter()
            .filter_entry(|entry| !is_ignored_path(entry.path()))
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "rs"))
        {
            let file_path = entry.path();
            let source = std::fs::read_to_string(file_path)
                .unwrap_or_else(|err| panic!("failed to read {}: {err}", file_path.display()));
            let parsed = syn::parse_file(&source)
                .unwrap_or_else(|err| panic!("failed to parse {}: {err}", file_path.display()));
            let relative_path = file_path
                .strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")))
                .unwrap_or(file_path)
                .to_path_buf();

            collect_items(&parsed.items, &relative_path, &mut summary);
        }

        summary.outputs.sort_by(|left, right| {
            left.kind
                .cmp(&right.kind)
                .then(left.rust_type.cmp(&right.rust_type))
        });

        for output in &mut summary.outputs {
            output.tuple_field_type = summary
                .tuple_field_types_by_struct
                .get(&output.rust_type)
                .cloned();
        }

        summary
    }

    fn is_ignored_path(path: &Path) -> bool {
        path.components().any(|component| {
            let component = component.as_os_str();
            component == "target" || component == ".git"
        })
    }

    fn collect_items(items: &[Item], file: &Path, summary: &mut SourceSummary) {
        for item in items {
            match item {
                Item::Struct(item) => collect_struct(item, file, summary),
                Item::Impl(item) => collect_impl(item, file, summary),
                Item::Mod(item) => {
                    if let Some((_, items)) = &item.content {
                        collect_items(items, file, summary);
                    }
                }
                _ => {}
            }
        }
    }

    fn collect_struct(item: &syn::ItemStruct, file: &Path, summary: &mut SourceSummary) {
        if let syn::Fields::Unnamed(fields) = &item.fields
            && fields.unnamed.len() == 1
            && let Some(ty) = fields
                .unnamed
                .first()
                .map(|field| field.ty.to_token_stream().to_string())
        {
            summary
                .tuple_field_types_by_struct
                .insert(item.ident.to_string(), ty);
        }

        let _ = file;
    }

    fn collect_impl(item: &ItemImpl, file: &Path, summary: &mut SourceSummary) {
        let Some(trait_name) = item
            .trait_
            .as_ref()
            .and_then(|(_, path, _)| path.segments.last())
            .map(|segment| segment.ident.to_string())
        else {
            return;
        };

        let Some(rust_type) = type_name(&item.self_ty) else {
            return;
        };

        if trait_name.as_str() == "StructuredOutput" {
            let mut kind = None;

            for impl_item in &item.items {
                if let ImplItem::Const(constant) = impl_item
                    && constant.ident == "KIND"
                {
                    kind = string_literal(&constant.expr);
                }
            }

            summary.outputs.push(OutputImpl {
                rust_type,
                kind: kind.unwrap_or_else(|| "<missing-kind>".to_string()),
                file: file.to_path_buf(),
                tuple_field_type: None,
            });
        }
    }

    fn type_name(ty: &Type) -> Option<String> {
        match ty {
            Type::Path(path) => path
                .path
                .segments
                .last()
                .map(|segment| segment.ident.to_string()),
            _ => None,
        }
    }

    fn string_literal(expr: &Expr) -> Option<String> {
        match expr {
            Expr::Lit(lit) => match &lit.lit {
                Lit::Str(value) => Some(value.value()),
                _ => None,
            },
            _ => None,
        }
    }

    fn is_valid_kind(kind: &str) -> bool {
        let parts = kind.split('.').collect::<Vec<_>>();

        !parts.is_empty()
            && parts.iter().all(|part| {
                !part.is_empty()
                    && part.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                    })
                    && part
                        .bytes()
                        .next()
                        .is_some_and(|byte| byte.is_ascii_lowercase())
            })
    }

    fn is_known_non_object_type(ty: &str) -> bool {
        let compact = ty.replace(' ', "");

        if compact.starts_with("Vec<")
            || compact.starts_with("Option<")
            || compact.starts_with("HashSet<")
            || compact.starts_with("BTreeSet<")
            || compact.starts_with('[')
        {
            return true;
        }

        matches!(
            compact.as_str(),
            "String"
                | "str"
                | "&str"
                | "bool"
                | "u8"
                | "u16"
                | "u32"
                | "u64"
                | "usize"
                | "i8"
                | "i16"
                | "i32"
                | "i64"
                | "isize"
                | "f32"
                | "f64"
        )
    }

    fn render_markdown_summary(summary: &[OutputImpl]) -> String {
        let mut output = String::new();

        output.push_str("# CLI Output Source Summary\n\n");
        output.push_str(
            "Generated from Rust source. Review `$type` names and Rust type mappings.\n\n",
        );

        output.push_str("## Outputs\n\n");
        output.push_str("| `$type` | Rust Type | Source |\n");
        output.push_str("|---|---|---|\n");

        for item in summary {
            output.push_str(&format!(
                "| `{}` | `{}` | `{}` |\n",
                escape_table_cell(&item.type_name()),
                escape_table_cell(&item.rust_type),
                item.file.display(),
            ));
        }

        output
    }

    fn escape_table_cell(value: &str) -> String {
        value.replace('|', "\\|").replace('\n', " ")
    }

    fn arb_registered_output_document() -> BoxedStrategy<Value> {
        let strategies = STRUCTURED_OUTPUT_TEST_REGISTRY
            .iter()
            .map(|entry| (entry.arbitrary)())
            .collect::<Vec<_>>();
        proptest::strategy::Union::new(strategies).boxed()
    }

    fn serialized_output<T>(strategy: impl Strategy<Value = T> + 'static) -> OutputDocumentStrategy
    where
        T: StructuredOutput + 'static,
    {
        strategy
            .prop_map(|output| {
                to_structured_output_value(output).expect("generated DTO should serialize")
            })
            .boxed()
    }

    fn empty_deployment_diff() -> golem_common::model::diff::DeploymentDiff {
        golem_common::model::diff::DeploymentDiff {
            components: BTreeMap::new(),
            http_api_deployments: BTreeMap::new(),
            mcp_deployments: BTreeMap::new(),
        }
    }

    fn arb_small_string() -> BoxedStrategy<String> {
        any::<u128>()
            .prop_map(|value| uuid::Uuid::from_u128(value).to_string())
            .boxed()
    }

    fn arb_uuid() -> BoxedStrategy<uuid::Uuid> {
        any::<u128>().prop_map(uuid::Uuid::from_u128).boxed()
    }

    fn arb_small_u64() -> BoxedStrategy<u64> {
        (0u64..1000).boxed()
    }

    fn arb_timestamp_string() -> BoxedStrategy<String> {
        arb_datetime().prop_map(|value| value.to_rfc3339()).boxed()
    }

    fn arb_timestamp() -> BoxedStrategy<golem_common::model::Timestamp> {
        arb_timestamp_string()
            .prop_map(|value| value.parse().expect("generated timestamp should parse"))
            .boxed()
    }

    fn arb_url_string() -> BoxedStrategy<String> {
        arb_small_string()
            .prop_map(|path| format!("https://example.com/{path}"))
            .boxed()
    }

    fn arb_datetime() -> BoxedStrategy<chrono::DateTime<chrono::Utc>> {
        (0i64..4_102_444_800i64)
            .prop_map(|seconds| {
                chrono::DateTime::from_timestamp(seconds, 0)
                    .expect("generated timestamp should be in range")
            })
            .boxed()
    }

    fn fixed_datetime() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("1970-01-01T00:00:00Z")
            .expect("fixed timestamp should parse")
            .with_timezone(&chrono::Utc)
    }

    fn arb_hash() -> BoxedStrategy<golem_common::model::diff::Hash> {
        any::<u128>()
            .prop_map(|value| {
                golem_common::model::diff::Hash::new(blake3::hash(&value.to_le_bytes()))
            })
            .boxed()
    }

    fn arb_agent_status() -> BoxedStrategy<golem_common::model::AgentStatus> {
        prop_oneof![
            Just(golem_common::model::AgentStatus::Running),
            Just(golem_common::model::AgentStatus::Idle),
            Just(golem_common::model::AgentStatus::Suspended),
            Just(golem_common::model::AgentStatus::Interrupted),
            Just(golem_common::model::AgentStatus::Retrying),
            Just(golem_common::model::AgentStatus::Failed),
            Just(golem_common::model::AgentStatus::Exited),
        ]
        .boxed()
    }

    fn sample_public_oplog_entries() -> Vec<golem_common::model::oplog::PublicOplogEntry> {
        use golem_common::base_model::retry_policy::{
            ApiImmediatePolicy, ApiPredicate, ApiPredicateTrue, ApiRetryPolicy,
        };
        use golem_common::model::component::{ComponentId, ComponentRevision, PluginPriority};
        use golem_common::model::environment::EnvironmentId;
        use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantId;
        use golem_common::model::invocation_context::{SpanId, TraceId};
        use golem_common::model::oplog::public_oplog_entry::*;
        use golem_common::model::oplog::*;
        use golem_common::model::regions::OplogRegion;
        use golem_common::model::{AgentId, Empty, IdempotencyKey, Timestamp};
        use golem_common::schema::{SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue};
        use std::iter::FromIterator;
        use uuid::Uuid;

        fn timestamp() -> Timestamp {
            Timestamp::from(0)
        }

        fn component_id() -> ComponentId {
            ComponentId(Uuid::parse_str("13a5c8d4-f05e-4e23-b982-f4d413e181cb").unwrap())
        }

        fn agent_id(name: &str) -> AgentId {
            AgentId {
                component_id: component_id(),
                agent_id: name.to_string(),
            }
        }

        fn plugin(priority: i32) -> PluginInstallationDescription {
            PluginInstallationDescription {
                environment_plugin_grant_id: EnvironmentPluginGrantId::new(),
                plugin_priority: PluginPriority(priority),
                plugin_name: "generated-plugin".to_string(),
                plugin_version: "1.0.0".to_string(),
                parameters: BTreeMap::from_iter([("key".to_string(), "value".to_string())]),
            }
        }

        fn typed_string_value(value: &str) -> TypedSchemaValue {
            TypedSchemaValue::new(
                SchemaGraph::anonymous(SchemaType::string()),
                SchemaValue::String(value.to_string()),
            )
        }

        fn typed_u64_list_value(values: Vec<u64>) -> TypedSchemaValue {
            TypedSchemaValue::new(
                SchemaGraph::anonymous(SchemaType::list(SchemaType::u64())),
                SchemaValue::List {
                    elements: values.into_iter().map(SchemaValue::U64).collect(),
                },
            )
        }

        fn span_context() -> Vec<Vec<PublicSpanData>> {
            vec![vec![PublicSpanData::LocalSpan(PublicLocalSpanData {
                span_id: SpanId::generate(),
                start: timestamp(),
                parent_id: None,
                linked_context: Some(1),
                attributes: vec![PublicAttribute {
                    key: "component".to_string(),
                    value: PublicAttributeValue::String(StringAttributeValue {
                        value: "generated".to_string(),
                    }),
                }],
                inherited: true,
            })]]
        }

        fn method_invocation() -> PublicAgentInvocation {
            PublicAgentInvocation::AgentMethodInvocation(AgentMethodInvocationParameters {
                idempotency_key: IdempotencyKey::new("method-key".to_string()),
                method_name: "generated-method".to_string(),
                function_input: typed_string_value("input"),
                trace_id: TraceId::generate(),
                trace_states: vec!["trace-state".to_string()],
                invocation_context: span_context(),
            })
        }

        fn raw_snapshot() -> PublicSnapshotData {
            PublicSnapshotData::Raw(RawSnapshotData {
                data: vec![1, 2, 3],
                mime_type: "application/octet-stream".to_string(),
            })
        }

        fn json_snapshot() -> PublicSnapshotData {
            PublicSnapshotData::Json(JsonSnapshotData {
                data: json!({ "counter": 42 }),
            })
        }

        fn multipart_snapshot() -> PublicSnapshotData {
            PublicSnapshotData::Multipart(MultipartSnapshotData {
                mime_type: "multipart/mixed; boundary=generated".to_string(),
                parts: vec![
                    MultipartSnapshotPart {
                        name: "state".to_string(),
                        content_type: "application/json".to_string(),
                        data: MultipartPartData::Json(JsonSnapshotData {
                            data: json!({ "state": "ok" }),
                        }),
                    },
                    MultipartSnapshotPart {
                        name: "bytes".to_string(),
                        content_type: "application/octet-stream".to_string(),
                        data: MultipartPartData::Raw(RawSnapshotData {
                            data: vec![4, 5, 6],
                            mime_type: "application/octet-stream".to_string(),
                        }),
                    },
                ],
            })
        }

        let retry_policy_state = PublicRetryPolicyState::AndThen(PublicRetryPolicyStateAndThen {
            left: Box::new(PublicRetryPolicyState::Counter(
                PublicRetryPolicyStateCounter { count: 2 },
            )),
            right: Box::new(PublicRetryPolicyState::Terminal(Empty {})),
            on_right: true,
        });

        let retry_policy = PublicNamedRetryPolicy {
            name: "generated-retry".to_string(),
            priority: 10,
            predicate: ApiPredicate::True(ApiPredicateTrue {}),
            policy: ApiRetryPolicy::Immediate(ApiImmediatePolicy {}),
        };

        vec![
            PublicOplogEntry::Create(CreateParams {
                timestamp: timestamp(),
                agent_id: agent_id("generated-agent"),
                agent_mode: golem_common::model::agent::AgentMode::Durable,
                component_revision: ComponentRevision::new(1).unwrap(),
                env: BTreeMap::from_iter([("ENV".to_string(), "value".to_string())]),
                created_by: golem_common::model::account::AccountId::new(),
                local_agent_config: vec![PublicTypedAgentConfigEntry {
                    path: vec!["config".to_string()],
                    value: typed_string_value("configured"),
                }],
                environment_id: EnvironmentId::new(),
                parent: Some(agent_id("parent-agent")),
                component_size: 10,
                initial_total_linear_memory_size: 20,
                initial_active_plugins: BTreeSet::from_iter([plugin(0)]),
                original_phantom_id: Some(
                    Uuid::parse_str("23a5c8d4-f05e-4e23-b982-f4d413e181cb").unwrap(),
                ),
                instance_id: Uuid::parse_str("33a5c8d4-f05e-4e23-b982-f4d413e181cb").unwrap(),
            }),
            PublicOplogEntry::Start(StartParams {
                timestamp: timestamp(),
                parent_start_index: Some(OplogIndex::from_u64(1)),
                function_name: "wasi:keyvalue/store.{get}".to_string(),
                request: Some(typed_string_value("request")),
                durable_function_type: PublicDurableFunctionType::WriteRemoteBatched(
                    WriteRemoteBatchedParameters {
                        index: Some(OplogIndex::from_u64(1)),
                    },
                ),
            }),
            PublicOplogEntry::End(EndParams {
                timestamp: timestamp(),
                start_index: OplogIndex::from_u64(1),
                response: Some(typed_u64_list_value(vec![1])),
                forced_commit: false,
            }),
            PublicOplogEntry::Cancelled(CancelledParams {
                timestamp: timestamp(),
                start_index: OplogIndex::from_u64(2),
                partial: Some(typed_string_value("partial")),
            }),
            PublicOplogEntry::AgentInvocationStarted(AgentInvocationStartedParams {
                timestamp: timestamp(),
                invocation: method_invocation(),
            }),
            PublicOplogEntry::AgentInvocationFinished(AgentInvocationFinishedParams {
                timestamp: timestamp(),
                result: PublicAgentInvocationResult::AgentMethod(AgentInvocationOutputParameters {
                    output: typed_string_value("output"),
                }),
                method_name: Some("generated-method".to_string()),
                consumed_fuel: 100,
                component_revision: ComponentRevision::new(1).unwrap(),
            }),
            PublicOplogEntry::Suspend(SuspendParams {
                timestamp: timestamp(),
            }),
            PublicOplogEntry::Error(ErrorParams {
                timestamp: timestamp(),
                error: "generated error".to_string(),
                retry_from: OplogIndex::INITIAL,
                inside_atomic_region: false,
                retry_policy_state: Some(retry_policy_state),
            }),
            PublicOplogEntry::NoOp(NoOpParams {
                timestamp: timestamp(),
            }),
            PublicOplogEntry::Jump(JumpParams {
                timestamp: timestamp(),
                jump: OplogRegion {
                    start: OplogIndex::from_u64(1),
                    end: OplogIndex::from_u64(2),
                },
            }),
            PublicOplogEntry::Interrupted(InterruptedParams {
                timestamp: timestamp(),
            }),
            PublicOplogEntry::Exited(ExitedParams {
                timestamp: timestamp(),
            }),
            PublicOplogEntry::BeginAtomicRegion(BeginAtomicRegionParams {
                timestamp: timestamp(),
            }),
            PublicOplogEntry::EndAtomicRegion(EndAtomicRegionParams {
                timestamp: timestamp(),
                begin_index: OplogIndex::from_u64(1),
            }),
            PublicOplogEntry::PendingAgentInvocation(PendingAgentInvocationParams {
                timestamp: timestamp(),
                invocation: PublicAgentInvocation::AgentInitialization(
                    AgentInitializationParameters {
                        idempotency_key: IdempotencyKey::new("init-key".to_string()),
                        constructor_parameters: typed_string_value("constructor"),
                        trace_id: TraceId::generate(),
                        trace_states: vec![],
                        invocation_context: span_context(),
                    },
                ),
            }),
            PublicOplogEntry::PendingUpdate(PendingUpdateParams {
                timestamp: timestamp(),
                target_revision: ComponentRevision::new(2).unwrap(),
                description: PublicUpdateDescription::SnapshotBased(
                    SnapshotBasedUpdateParameters {
                        payload: vec![7, 8, 9],
                        mime_type: "application/octet-stream".to_string(),
                    },
                ),
            }),
            PublicOplogEntry::SuccessfulUpdate(SuccessfulUpdateParams {
                timestamp: timestamp(),
                target_revision: ComponentRevision::new(2).unwrap(),
                new_component_size: 30,
                new_active_plugins: BTreeSet::from_iter([plugin(1)]),
            }),
            PublicOplogEntry::FailedUpdate(FailedUpdateParams {
                timestamp: timestamp(),
                target_revision: ComponentRevision::new(3).unwrap(),
                details: None,
            }),
            PublicOplogEntry::GrowMemory(GrowMemoryParams {
                timestamp: timestamp(),
                delta: 64,
            }),
            PublicOplogEntry::FilesystemStorageUsageUpdate(FilesystemStorageUsageUpdateParams {
                timestamp: timestamp(),
                delta: -5,
            }),
            PublicOplogEntry::CreateResource(CreateResourceParams {
                timestamp: timestamp(),
                id: AgentResourceId(1),
                name: "resource".to_string(),
                owner: "owner".to_string(),
            }),
            PublicOplogEntry::DropResource(DropResourceParams {
                timestamp: timestamp(),
                id: AgentResourceId(1),
                name: "resource".to_string(),
                owner: "owner".to_string(),
            }),
            PublicOplogEntry::Log(LogParams {
                timestamp: timestamp(),
                level: LogLevel::Info,
                context: "generated".to_string(),
                message: "message".to_string(),
            }),
            PublicOplogEntry::Restart(RestartParams {
                timestamp: timestamp(),
            }),
            PublicOplogEntry::ActivatePlugin(ActivatePluginParams {
                timestamp: timestamp(),
                plugin: plugin(2),
            }),
            PublicOplogEntry::DeactivatePlugin(DeactivatePluginParams {
                timestamp: timestamp(),
                plugin: plugin(3),
            }),
            PublicOplogEntry::Revert(RevertParams {
                timestamp: timestamp(),
                dropped_region: OplogRegion {
                    start: OplogIndex::from_u64(5),
                    end: OplogIndex::from_u64(10),
                },
            }),
            PublicOplogEntry::CancelPendingInvocation(CancelPendingInvocationParams {
                timestamp: timestamp(),
                idempotency_key: IdempotencyKey::new("cancel-key".to_string()),
            }),
            PublicOplogEntry::StartSpan(StartSpanParams {
                timestamp: timestamp(),
                span_id: SpanId::generate(),
                parent_id: Some(SpanId::generate()),
                linked_context: Some(SpanId::generate()),
                attributes: vec![PublicAttribute {
                    key: "http.method".to_string(),
                    value: PublicAttributeValue::String(StringAttributeValue {
                        value: "GET".to_string(),
                    }),
                }],
            }),
            PublicOplogEntry::FinishSpan(FinishSpanParams {
                timestamp: timestamp(),
                span_id: SpanId::generate(),
            }),
            PublicOplogEntry::SetSpanAttribute(SetSpanAttributeParams {
                timestamp: timestamp(),
                span_id: SpanId::generate(),
                key: "http.status_code".to_string(),
                value: PublicAttributeValue::String(StringAttributeValue {
                    value: "200".to_string(),
                }),
            }),
            PublicOplogEntry::ChangePersistenceLevel(ChangePersistenceLevelParams {
                timestamp: timestamp(),
                persistence_level: PersistenceLevel::Smart,
            }),
            PublicOplogEntry::BeginRemoteTransaction(BeginRemoteTransactionParams {
                timestamp: timestamp(),
                transaction_id: golem_common::model::TransactionId::new("txn-1".to_string()),
            }),
            PublicOplogEntry::PreCommitRemoteTransaction(PreCommitRemoteTransactionParams {
                timestamp: timestamp(),
                begin_index: OplogIndex::from_u64(11),
            }),
            PublicOplogEntry::PreRollbackRemoteTransaction(PreRollbackRemoteTransactionParams {
                timestamp: timestamp(),
                begin_index: OplogIndex::from_u64(11),
            }),
            PublicOplogEntry::CommittedRemoteTransaction(CommittedRemoteTransactionParams {
                timestamp: timestamp(),
                begin_index: OplogIndex::from_u64(11),
            }),
            PublicOplogEntry::RolledBackRemoteTransaction(RolledBackRemoteTransactionParams {
                timestamp: timestamp(),
                begin_index: OplogIndex::from_u64(11),
            }),
            PublicOplogEntry::Snapshot(SnapshotParams {
                timestamp: timestamp(),
                data: raw_snapshot(),
            }),
            PublicOplogEntry::Snapshot(SnapshotParams {
                timestamp: timestamp(),
                data: json_snapshot(),
            }),
            PublicOplogEntry::Snapshot(SnapshotParams {
                timestamp: timestamp(),
                data: multipart_snapshot(),
            }),
            PublicOplogEntry::OplogProcessorCheckpoint(OplogProcessorCheckpointParams {
                timestamp: timestamp(),
                plugin: plugin(4),
                target_agent_id: agent_id("target-agent"),
                confirmed_up_to: OplogIndex::from_u64(20),
                sending_up_to: OplogIndex::from_u64(21),
                last_batch_start: OplogIndex::from_u64(19),
            }),
            PublicOplogEntry::SetRetryPolicy(SetRetryPolicyParams {
                timestamp: timestamp(),
                policy: retry_policy,
            }),
            PublicOplogEntry::RemoveRetryPolicy(RemoveRetryPolicyParams {
                timestamp: timestamp(),
                name: "generated-retry".to_string(),
            }),
            PublicOplogEntry::AgentInvocationFinished(AgentInvocationFinishedParams {
                timestamp: timestamp(),
                result: PublicAgentInvocationResult::SaveSnapshot(SaveSnapshotResultParameters {
                    snapshot: json_snapshot(),
                }),
                method_name: Some("generated-method".to_string()),
                consumed_fuel: 101,
                component_revision: ComponentRevision::new(4).unwrap(),
            }),
            PublicOplogEntry::PendingAgentInvocation(PendingAgentInvocationParams {
                timestamp: timestamp(),
                invocation: PublicAgentInvocation::LoadSnapshot(LoadSnapshotParameters {
                    snapshot: multipart_snapshot(),
                }),
            }),
            PublicOplogEntry::PendingAgentInvocation(PendingAgentInvocationParams {
                timestamp: timestamp(),
                invocation: PublicAgentInvocation::SaveSnapshot(Empty {}),
            }),
            PublicOplogEntry::PendingAgentInvocation(PendingAgentInvocationParams {
                timestamp: timestamp(),
                invocation: PublicAgentInvocation::ProcessOplogEntries(
                    ProcessOplogEntriesParameters {
                        idempotency_key: IdempotencyKey::new("process-key".to_string()),
                    },
                ),
            }),
            PublicOplogEntry::PendingAgentInvocation(PendingAgentInvocationParams {
                timestamp: timestamp(),
                invocation: PublicAgentInvocation::ManualUpdate(ManualUpdateParameters {
                    target_revision: ComponentRevision::new(5).unwrap(),
                }),
            }),
            PublicOplogEntry::AgentInvocationFinished(AgentInvocationFinishedParams {
                timestamp: timestamp(),
                result: PublicAgentInvocationResult::LoadSnapshot(FallibleResultParameters {
                    error: Some("load failed".to_string()),
                }),
                method_name: Some("generated-method".to_string()),
                consumed_fuel: 102,
                component_revision: ComponentRevision::new(5).unwrap(),
            }),
            PublicOplogEntry::AgentInvocationFinished(AgentInvocationFinishedParams {
                timestamp: timestamp(),
                result: PublicAgentInvocationResult::ProcessOplogEntries(
                    ProcessOplogEntriesResultParameters { error: None },
                ),
                method_name: Some("generated-method".to_string()),
                consumed_fuel: 103,
                component_revision: ComponentRevision::new(6).unwrap(),
            }),
            PublicOplogEntry::AgentInvocationFinished(AgentInvocationFinishedParams {
                timestamp: timestamp(),
                result: PublicAgentInvocationResult::ManualUpdate(Empty {}),
                method_name: Some("generated-method".to_string()),
                consumed_fuel: 104,
                component_revision: ComponentRevision::new(7).unwrap(),
            }),
            PublicOplogEntry::PendingUpdate(PendingUpdateParams {
                timestamp: timestamp(),
                target_revision: ComponentRevision::new(8).unwrap(),
                description: PublicUpdateDescription::Automatic(Empty {}),
            }),
        ]
    }

    fn arb_build_result() -> OutputDocumentStrategy {
        serialized_output(
            any::<bool>()
                .prop_map(|built| crate::model::text::action_result::BuildResult { built }),
        )
    }

    fn arb_clean_result() -> OutputDocumentStrategy {
        serialized_output(
            any::<bool>()
                .prop_map(|cleaned| crate::model::text::action_result::CleanResult { cleaned }),
        )
    }

    fn arb_deploy_result() -> OutputDocumentStrategy {
        serialized_output(
            any::<bool>().prop_map(|deployed| {
                crate::model::text::action_result::DeployResultView { deployed }
            }),
        )
    }

    fn arb_generate_bridge_result() -> OutputDocumentStrategy {
        serialized_output(any::<bool>().prop_map(|generated| {
            crate::model::text::action_result::GenerateBridgeResult { generated }
        }))
    }

    fn arb_agent_type_get_result() -> OutputDocumentStrategy {
        serialized_output(
            (arb_small_string(), arb_small_string(), arb_small_string()).prop_map(
                |(agent_type, constructor, description)| crate::model::agent::view::AgentTypeView {
                    agent_type,
                    constructor,
                    description,
                },
            ),
        )
    }

    fn arb_agent_type_list_result() -> OutputDocumentStrategy {
        serialized_output(
            proptest::collection::vec(arb_deployed_registered_agent_type(), 0..3).prop_map(
                |agent_types| crate::model::text::agent::AgentTypeListView { agent_types },
            ),
        )
    }

    fn arb_deployed_registered_agent_type()
    -> BoxedStrategy<golem_common::model::agent::DeployedRegisteredAgentType> {
        (
            arb_agent_type(),
            arb_uuid(),
            arb_small_u64(),
            arb_small_string(),
            arb_uuid(),
            arb_small_string(),
            proptest::option::of(arb_small_string()),
        )
            .prop_map(
                |(
                    agent_type,
                    component_id,
                    component_revision,
                    component_name,
                    account_id,
                    account_email,
                    webhook_prefix_authority_and_path,
                )| golem_common::model::agent::DeployedRegisteredAgentType {
                    agent_type,
                    implemented_by: golem_common::model::agent::RegisteredAgentTypeImplementer {
                        component_id: golem_common::model::component::ComponentId(component_id),
                        component_revision: golem_common::model::component::ComponentRevision::new(
                            component_revision,
                        )
                        .expect("generated revision should be valid"),
                        component_name,
                        account_id: golem_common::model::account::AccountId(account_id),
                        account_email: golem_common::model::account::AccountEmail::new(
                            account_email,
                        ),
                    },
                    webhook_prefix_authority_and_path,
                },
            )
            .boxed()
    }

    fn arb_agent_type() -> BoxedStrategy<golem_common::schema::agent::AgentTypeSchema> {
        (
            arb_agent_type_name(),
            arb_small_string(),
            arb_small_string(),
            arb_agent_constructor(),
            proptest::collection::vec(arb_agent_method(), 1..3),
            proptest::collection::vec(arb_agent_dependency(), 1..2),
            arb_agent_mode(),
            proptest::option::of(arb_http_mount_details()),
            arb_snapshotting(),
            proptest::collection::vec(arb_agent_config_declaration(), 1..3),
        )
            .prop_map(
                |(
                    type_name,
                    description,
                    source_language,
                    constructor,
                    methods,
                    dependencies,
                    mode,
                    http_mount,
                    snapshotting,
                    config,
                )| {
                    let methods = if mode == golem_common::model::agent::AgentMode::Ephemeral {
                        methods
                            .into_iter()
                            .map(|mut method| {
                                method.read_only = None;
                                method
                            })
                            .collect()
                    } else {
                        methods
                    };

                    golem_common::schema::agent::AgentTypeSchema {
                        type_name,
                        description,
                        source_language,
                        schema: golem_common::schema::SchemaGraph::empty(),
                        constructor,
                        methods,
                        dependencies,
                        mode,
                        http_mount,
                        snapshotting,
                        config,
                    }
                },
            )
            .boxed()
    }

    fn arb_agent_constructor() -> BoxedStrategy<golem_common::schema::agent::AgentConstructorSchema>
    {
        (
            proptest::option::of(arb_small_string()),
            arb_small_string(),
            proptest::option::of(arb_small_string()),
            any::<u8>(),
        )
            .prop_map(|(name, description, prompt_hint, schema_flavor)| {
                golem_common::schema::agent::AgentConstructorSchema {
                    name,
                    description,
                    prompt_hint,
                    input_schema: input_schema_value(schema_flavor),
                }
            })
            .boxed()
    }

    fn arb_agent_method() -> BoxedStrategy<golem_common::schema::agent::AgentMethodSchema> {
        (
            arb_small_string(),
            arb_small_string(),
            proptest::option::of(arb_small_string()),
            any::<u8>(),
            any::<u8>(),
            proptest::collection::vec(arb_http_endpoint_details(), 1..3),
            proptest::option::of(arb_read_only_config()),
        )
            .prop_map(
                |(
                    name,
                    description,
                    prompt_hint,
                    input_schema_flavor,
                    output_schema_flavor,
                    http_endpoint,
                    read_only,
                )| {
                    golem_common::schema::agent::AgentMethodSchema {
                        name,
                        description,
                        prompt_hint,
                        input_schema: input_schema_value(input_schema_flavor),
                        output_schema: output_schema_value(output_schema_flavor),
                        http_endpoint,
                        read_only,
                    }
                },
            )
            .boxed()
    }

    fn arb_agent_dependency() -> BoxedStrategy<golem_common::schema::agent::AgentDependencySchema> {
        (
            arb_small_string(),
            proptest::option::of(arb_small_string()),
            arb_agent_constructor(),
            proptest::collection::vec(arb_agent_method(), 1..2),
        )
            .prop_map(|(type_name, description, constructor, methods)| {
                golem_common::schema::agent::AgentDependencySchema {
                    type_name,
                    description,
                    schema: golem_common::schema::SchemaGraph::empty(),
                    constructor,
                    methods,
                }
            })
            .boxed()
    }

    fn arb_agent_mode() -> BoxedStrategy<golem_common::model::agent::AgentMode> {
        prop_oneof![
            Just(golem_common::model::agent::AgentMode::Durable),
            Just(golem_common::model::agent::AgentMode::Ephemeral),
        ]
        .boxed()
    }

    fn schema_type_value(flavor: u8) -> golem_common::schema::SchemaType {
        match flavor % 4 {
            0 => golem_common::schema::SchemaType::string(),
            1 => golem_common::schema::SchemaType::u64(),
            2 => golem_common::schema::SchemaType::bool(),
            _ => golem_common::schema::SchemaType::list(golem_common::schema::SchemaType::u64()),
        }
    }

    fn input_schema_value(flavor: u8) -> golem_common::schema::agent::InputSchema {
        golem_common::schema::agent::InputSchema::parameters([
            golem_common::schema::agent::NamedField::user_supplied(
                "value",
                schema_type_value(flavor),
            ),
        ])
    }

    fn output_schema_value(flavor: u8) -> golem_common::schema::agent::OutputSchema {
        golem_common::schema::agent::OutputSchema::Single(Box::new(schema_type_value(flavor)))
    }

    fn arb_read_only_config() -> BoxedStrategy<golem_common::model::agent::ReadOnlyConfig> {
        (arb_cache_policy(), any::<bool>())
            .prop_map(
                |(cache_policy, uses_principal)| golem_common::model::agent::ReadOnlyConfig {
                    cache_policy,
                    uses_principal,
                },
            )
            .boxed()
    }

    fn arb_cache_policy() -> BoxedStrategy<golem_common::model::agent::CachePolicy> {
        prop_oneof![
            Just(golem_common::model::agent::CachePolicy::NoCache(
                golem_common::model::Empty {}
            )),
            Just(golem_common::model::agent::CachePolicy::UntilWrite(
                golem_common::model::Empty {}
            )),
            arb_small_u64().prop_map(|duration_nanos| {
                golem_common::model::agent::CachePolicy::Ttl(
                    golem_common::model::agent::CachePolicyTtl { duration_nanos },
                )
            }),
        ]
        .boxed()
    }

    fn arb_snapshotting() -> BoxedStrategy<golem_common::model::agent::Snapshotting> {
        prop_oneof![
            Just(golem_common::model::agent::Snapshotting::Disabled(
                golem_common::model::Empty {}
            )),
            Just(golem_common::model::agent::Snapshotting::Enabled(
                golem_common::model::agent::SnapshottingConfig::Default(
                    golem_common::model::Empty {},
                )
            )),
            arb_small_u64().prop_map(|duration_nanos| {
                golem_common::model::agent::Snapshotting::Enabled(
                    golem_common::model::agent::SnapshottingConfig::Periodic(
                        golem_common::model::agent::SnapshottingPeriodic { duration_nanos },
                    ),
                )
            }),
            any::<u16>().prop_map(|count| {
                golem_common::model::agent::Snapshotting::Enabled(
                    golem_common::model::agent::SnapshottingConfig::EveryNInvocation(
                        golem_common::model::agent::SnapshottingEveryNInvocation { count },
                    ),
                )
            }),
        ]
        .boxed()
    }

    fn arb_agent_config_declaration()
    -> BoxedStrategy<golem_common::schema::agent::AgentConfigDeclarationSchema> {
        (
            prop_oneof![
                Just(golem_common::model::agent::AgentConfigSource::Local),
                Just(golem_common::model::agent::AgentConfigSource::Secret),
            ],
            proptest::collection::vec(arb_small_string(), 1..3),
            prop_oneof![
                Just(golem_common::schema::SchemaType::string()),
                Just(golem_common::schema::SchemaType::bool()),
                Just(golem_common::schema::SchemaType::u64()),
            ],
        )
            .prop_map(|(source, path, value_type)| {
                golem_common::schema::agent::AgentConfigDeclarationSchema {
                    source,
                    path,
                    value_type,
                }
            })
            .boxed()
    }

    fn arb_http_mount_details() -> BoxedStrategy<golem_common::model::agent::HttpMountDetails> {
        (
            proptest::collection::vec(arb_path_segment(), 0..2),
            proptest::option::of(any::<bool>().prop_map(|required| {
                golem_common::model::agent::AgentHttpAuthDetails { required }
            })),
            any::<bool>(),
            proptest::collection::vec(arb_small_string(), 0..2),
            proptest::collection::vec(arb_path_segment(), 0..2),
        )
            .prop_map(
                |(path_prefix, auth_details, phantom_agent, allowed_patterns, webhook_suffix)| {
                    golem_common::model::agent::HttpMountDetails {
                        path_prefix,
                        auth_details,
                        phantom_agent,
                        cors_options: golem_common::model::agent::CorsOptions { allowed_patterns },
                        webhook_suffix,
                    }
                },
            )
            .boxed()
    }

    fn arb_http_endpoint_details() -> BoxedStrategy<golem_common::model::agent::HttpEndpointDetails>
    {
        (
            arb_http_method(),
            proptest::collection::vec(arb_path_segment(), 0..2),
            proptest::collection::vec(
                (arb_small_string(), arb_small_string()).prop_map(
                    |(header_name, variable_name)| golem_common::model::agent::HeaderVariable {
                        header_name,
                        variable_name,
                    },
                ),
                0..2,
            ),
            proptest::collection::vec(
                (arb_small_string(), arb_small_string()).prop_map(
                    |(query_param_name, variable_name)| golem_common::model::agent::QueryVariable {
                        query_param_name,
                        variable_name,
                    },
                ),
                0..2,
            ),
            proptest::option::of(any::<bool>().prop_map(|required| {
                golem_common::model::agent::AgentHttpAuthDetails { required }
            })),
            proptest::collection::vec(arb_small_string(), 0..2),
        )
            .prop_map(
                |(
                    http_method,
                    path_suffix,
                    header_vars,
                    query_vars,
                    auth_details,
                    allowed_patterns,
                )| {
                    golem_common::model::agent::HttpEndpointDetails {
                        http_method,
                        path_suffix,
                        header_vars,
                        query_vars,
                        auth_details,
                        cors_options: golem_common::model::agent::CorsOptions { allowed_patterns },
                    }
                },
            )
            .boxed()
    }

    fn arb_http_method() -> BoxedStrategy<golem_common::model::agent::HttpMethod> {
        prop_oneof![
            Just(golem_common::model::agent::HttpMethod::Get(
                golem_common::model::Empty {}
            )),
            Just(golem_common::model::agent::HttpMethod::Head(
                golem_common::model::Empty {}
            )),
            Just(golem_common::model::agent::HttpMethod::Post(
                golem_common::model::Empty {}
            )),
            Just(golem_common::model::agent::HttpMethod::Put(
                golem_common::model::Empty {}
            )),
            Just(golem_common::model::agent::HttpMethod::Delete(
                golem_common::model::Empty {}
            )),
            Just(golem_common::model::agent::HttpMethod::Connect(
                golem_common::model::Empty {}
            )),
            Just(golem_common::model::agent::HttpMethod::Options(
                golem_common::model::Empty {}
            )),
            Just(golem_common::model::agent::HttpMethod::Trace(
                golem_common::model::Empty {}
            )),
            Just(golem_common::model::agent::HttpMethod::Patch(
                golem_common::model::Empty {}
            )),
            arb_small_string().prop_map(|value| {
                golem_common::model::agent::HttpMethod::Custom(
                    golem_common::model::agent::CustomHttpMethod { value },
                )
            }),
        ]
        .boxed()
    }

    fn arb_path_segment() -> BoxedStrategy<golem_common::model::agent::PathSegment> {
        prop_oneof![
            arb_small_string().prop_map(|value| {
                golem_common::model::agent::PathSegment::Literal(
                    golem_common::model::agent::LiteralSegment { value },
                )
            }),
            arb_small_string().prop_map(|variable_name| {
                golem_common::model::agent::PathSegment::PathVariable(
                    golem_common::model::agent::PathVariable { variable_name },
                )
            }),
            arb_small_string().prop_map(|variable_name| {
                golem_common::model::agent::PathSegment::RemainingPathVariable(
                    golem_common::model::agent::PathVariable { variable_name },
                )
            }),
            prop_oneof![
                Just(golem_common::model::agent::SystemVariable::AgentType),
                Just(golem_common::model::agent::SystemVariable::AgentVersion),
            ]
            .prop_map(|value| {
                golem_common::model::agent::PathSegment::SystemVariable(
                    golem_common::model::agent::SystemVariableSegment { value },
                )
            }),
        ]
        .boxed()
    }

    fn arb_agent_files_result() -> OutputDocumentStrategy {
        serialized_output(
            proptest::collection::vec(arb_file_node(), 0..6)
                .prop_map(|nodes| crate::model::text::worker::WorkerFilesView { nodes }),
        )
    }

    fn arb_file_node() -> BoxedStrategy<crate::model::text::worker::FileNodeView> {
        (
            arb_small_string(),
            arb_small_string(),
            arb_timestamp_string(),
            arb_timestamp_string(),
            arb_small_u64(),
        )
            .prop_map(|(name, last_modified, kind, permissions, size)| {
                crate::model::text::worker::FileNodeView {
                    name,
                    last_modified,
                    kind,
                    permissions,
                    size,
                }
            })
            .boxed()
    }

    fn arb_agent_get_result() -> OutputDocumentStrategy {
        serialized_output((arb_agent_metadata_view(), any::<bool>()).prop_map(
            |(metadata, precise)| crate::model::text::worker::WorkerGetView { metadata, precise },
        ))
    }

    fn arb_agent_invoke_result() -> OutputDocumentStrategy {
        (
            arb_small_string(),
            prop_oneof![Just(0u8), Just(1u8)],
            arb_value_and_type(),
            arb_format_string(),
        )
            .prop_map(|(idempotency_key, shape, result_json, result_format)| {
                to_structured_output_value(crate::model::invoke_result_view::InvokeResultView {
                    idempotency_key,
                    result_json: (shape == 1).then_some(result_json),
                    result: None,
                    result_format: (shape != 0).then_some(result_format.to_string()),
                    is_void_result: shape == 0,
                })
                .expect("generated invoke result should serialize")
            })
            .boxed()
    }

    fn arb_value_and_type() -> BoxedStrategy<golem_common::schema::TypedSchemaValue> {
        prop_oneof![
            arb_small_string().prop_map(|value| {
                golem_common::schema::TypedSchemaValue::new(
                    golem_common::schema::SchemaGraph::anonymous(
                        golem_common::schema::SchemaType::string(),
                    ),
                    golem_common::schema::SchemaValue::String(value),
                )
            }),
            any::<bool>().prop_map(|value| {
                golem_common::schema::TypedSchemaValue::new(
                    golem_common::schema::SchemaGraph::anonymous(
                        golem_common::schema::SchemaType::bool(),
                    ),
                    golem_common::schema::SchemaValue::Bool(value),
                )
            }),
            proptest::collection::vec(arb_small_u64(), 0..3).prop_map(|values| {
                golem_common::schema::TypedSchemaValue::new(
                    golem_common::schema::SchemaGraph::anonymous(
                        golem_common::schema::SchemaType::list(
                            golem_common::schema::SchemaType::u64(),
                        ),
                    ),
                    golem_common::schema::SchemaValue::List {
                        elements: values
                            .into_iter()
                            .map(golem_common::schema::SchemaValue::U64)
                            .collect(),
                    },
                )
            }),
        ]
        .boxed()
    }

    fn arb_agent_list_result() -> OutputDocumentStrategy {
        (
            proptest::collection::vec(arb_agent_metadata_view(), 0..5),
            proptest::collection::btree_map(arb_small_string(), arb_small_string(), 0..4),
        )
            .prop_map(
                |(agents, cursors)| crate::model::worker::AgentsMetadataResponseView {
                    agents,
                    cursors,
                },
            )
            .prop_map(|output| {
                to_structured_output_value(output).expect("generated DTO should serialize")
            })
            .boxed()
    }

    fn arb_agent_new_result() -> OutputDocumentStrategy {
        serialized_output(
            (arb_small_string(), proptest::option::of(arb_small_string())).prop_map(
                |(component_name, agent_name)| crate::model::text::worker::WorkerCreateView {
                    component_name: golem_common::model::component::ComponentName(component_name),
                    agent_name: agent_name.map(crate::model::worker::RawAgentId),
                },
            ),
        )
    }

    fn arb_agent_oplog_result() -> OutputDocumentStrategy {
        serialized_output(
            (
                arb_small_u64(),
                proptest::sample::select(sample_public_oplog_entries()),
            )
                .prop_map(|(index, entry)| {
                    crate::model::text::worker::AgentOplogEntryView { index, entry }
                }),
        )
    }

    fn arb_agent_stream_event() -> OutputDocumentStrategy {
        serialized_output(
            (
                arb_timestamp(),
                arb_agent_stream_event_kind(),
                arb_small_string(),
                arb_small_string(),
                arb_small_string(),
                proptest::option::of(arb_small_string()),
                proptest::option::of(arb_small_string()),
                proptest::option::of(arb_small_u64()),
                proptest::option::of(arb_small_string()),
            )
                .prop_map(
                    |(
                        timestamp,
                        kind,
                        level,
                        context,
                        message,
                        function_name,
                        idempotency_key,
                        number_of_missed_messages,
                        error,
                    )| crate::model::agent::stream::AgentStreamEvent {
                        timestamp,
                        kind,
                        level,
                        context,
                        message,
                        function_name,
                        idempotency_key,
                        number_of_missed_messages,
                        error,
                    },
                ),
        )
    }

    fn arb_agent_stream_event_kind()
    -> BoxedStrategy<crate::model::agent::stream::AgentStreamEventKind> {
        prop_oneof![
            Just(crate::model::agent::stream::AgentStreamEventKind::Log),
            Just(crate::model::agent::stream::AgentStreamEventKind::Stdout),
            Just(crate::model::agent::stream::AgentStreamEventKind::Stderr),
            Just(crate::model::agent::stream::AgentStreamEventKind::StreamClosed),
            Just(crate::model::agent::stream::AgentStreamEventKind::StreamError),
            Just(crate::model::agent::stream::AgentStreamEventKind::InvocationStarted),
            Just(crate::model::agent::stream::AgentStreamEventKind::InvocationFinished),
            Just(crate::model::agent::stream::AgentStreamEventKind::MissedMessages),
        ]
        .boxed()
    }

    fn arb_agent_update_result() -> OutputDocumentStrategy {
        serialized_output(
            (
                proptest::collection::vec(arb_worker_update_attempt(), 0..5),
                proptest::collection::vec(arb_worker_update_attempt(), 0..5),
            )
                .prop_map(|(triggered, failed)| {
                    crate::model::deploy::TryUpdateAllWorkersResult { triggered, failed }
                }),
        )
    }

    fn arb_worker_update_attempt() -> BoxedStrategy<crate::model::deploy::WorkerUpdateAttempt> {
        (
            arb_small_string(),
            arb_small_u64(),
            arb_small_string(),
            proptest::option::of(arb_small_string()),
        )
            .prop_map(|(component_name, target_revision, agent_name, error)| {
                crate::model::deploy::WorkerUpdateAttempt {
                    component_name: golem_common::model::component::ComponentName(component_name),
                    target_revision: golem_common::model::component::ComponentRevision::new(
                        target_revision,
                    )
                    .expect("generated revision should be valid"),
                    agent_name: crate::model::worker::RawAgentId(agent_name),
                    error,
                }
            })
            .boxed()
    }

    fn arb_agent_metadata_view() -> BoxedStrategy<crate::model::worker::AgentMetadataView> {
        (
            (
                arb_small_string(),
                arb_small_string(),
                arb_small_string(),
                arb_small_string(),
                proptest::collection::btree_map(arb_small_string(), arb_small_string(), 0..4),
                proptest::collection::btree_map(arb_small_string(), arb_small_string(), 0..4),
                proptest::collection::vec(arb_agent_config_entry_dto(), 0..4),
                proptest::collection::vec(arb_agent_config_entry_dto(), 0..4),
                arb_agent_status(),
            ),
            (
                arb_small_u64(),
                any::<u32>(),
                arb_small_u64(),
                proptest::collection::vec(arb_update_record(), 0..4),
                arb_timestamp_string(),
                proptest::option::of(arb_small_string()),
                arb_small_u64(),
                arb_small_u64(),
                proptest::collection::btree_map(
                    arb_small_string(),
                    arb_agent_resource_description(),
                    0..4,
                ),
            ),
        )
            .prop_map(|(left, right)| {
                let (
                    component_name,
                    agent_name,
                    created_by,
                    environment_id,
                    env,
                    default_env,
                    config,
                    default_config,
                    status,
                ) = left;
                let (
                    component_revision,
                    retry_count,
                    pending_invocation_count,
                    updates,
                    created_at,
                    last_error,
                    component_size,
                    total_linear_memory_size,
                    exported_resource_instances,
                ) = right;

                crate::model::worker::AgentMetadataView {
                    component_name: golem_common::model::component::ComponentName(component_name),
                    agent_name: crate::model::worker::RawAgentId(agent_name),
                    created_by: golem_common::model::account::AccountId(
                        uuid::Uuid::parse_str(&created_by).expect("generated UUID should parse"),
                    ),
                    environment_id: golem_common::model::environment::EnvironmentId(
                        uuid::Uuid::parse_str(&environment_id)
                            .expect("generated UUID should parse"),
                    ),
                    env: env.into_iter().collect(),
                    default_env: default_env.into_iter().collect(),
                    config,
                    default_config,
                    status,
                    component_revision: golem_common::model::component::ComponentRevision::new(
                        component_revision,
                    )
                    .expect("generated revision should be valid"),
                    retry_count,
                    pending_invocation_count,
                    updates,
                    created_at: created_at
                        .parse()
                        .expect("generated timestamp should parse"),
                    last_error,
                    component_size,
                    total_linear_memory_size,
                    exported_resource_instances: exported_resource_instances.into_iter().collect(),
                    source_language: crate::agent_id_display::SourceLanguage::default(),
                    secret_config_paths: BTreeSet::new(),
                }
            })
            .boxed()
    }

    fn arb_agent_config_entry_dto()
    -> BoxedStrategy<golem_common::model::worker::AgentConfigEntryDto> {
        (
            proptest::collection::vec(arb_small_string(), 1..4),
            arb_json_value(2),
        )
            .prop_map(
                |(path, value)| golem_common::model::worker::AgentConfigEntryDto {
                    path,
                    value: golem_common::base_model::json::NormalizedJsonValue(value),
                },
            )
            .boxed()
    }

    fn arb_update_record() -> BoxedStrategy<golem_common::model::worker::UpdateRecord> {
        prop_oneof![
            (arb_timestamp(), arb_small_u64()).prop_map(|(timestamp, target_revision)| {
                golem_common::model::worker::UpdateRecord::PendingUpdate(
                    golem_common::model::worker::PendingUpdate {
                        timestamp,
                        target_revision: golem_common::model::component::ComponentRevision::new(
                            target_revision,
                        )
                        .expect("generated revision should be valid"),
                    },
                )
            }),
            (arb_timestamp(), arb_small_u64()).prop_map(|(timestamp, target_revision)| {
                golem_common::model::worker::UpdateRecord::SuccessfulUpdate(
                    golem_common::model::worker::SuccessfulUpdate {
                        timestamp,
                        target_revision: golem_common::model::component::ComponentRevision::new(
                            target_revision,
                        )
                        .expect("generated revision should be valid"),
                    },
                )
            }),
            (
                arb_timestamp(),
                arb_small_u64(),
                proptest::option::of(arb_small_string()),
            )
                .prop_map(|(timestamp, target_revision, details)| {
                    golem_common::model::worker::UpdateRecord::FailedUpdate(
                        golem_common::model::worker::FailedUpdate {
                            timestamp,
                            target_revision:
                                golem_common::model::component::ComponentRevision::new(
                                    target_revision,
                                )
                                .expect("generated revision should be valid"),
                            details,
                        },
                    )
                }),
        ]
        .boxed()
    }

    fn arb_agent_resource_description()
    -> BoxedStrategy<golem_common::model::AgentResourceDescription> {
        (arb_timestamp(), arb_small_string(), arb_small_string())
            .prop_map(|(created_at, resource_owner, resource_name)| {
                golem_common::model::AgentResourceDescription {
                    created_at,
                    resource_owner,
                    resource_name,
                }
            })
            .boxed()
    }

    fn arb_agent_delete_result() -> OutputDocumentStrategy {
        serialized_output(
            (any::<bool>(), arb_small_string()).prop_map(|(deleted, agent)| {
                crate::model::text::action_result::AgentDeleteResult { deleted, agent }
            }),
        )
    }

    fn arb_agent_file_contents_result() -> OutputDocumentStrategy {
        serialized_output(
            (
                any::<bool>(),
                arb_small_string(),
                arb_small_string(),
                arb_small_string(),
                arb_small_u64(),
            )
                .prop_map(|(saved, agent, path, output_path, bytes)| {
                    crate::model::text::action_result::AgentFileContentsResult {
                        saved,
                        agent,
                        path,
                        output_path: output_path.into(),
                        bytes: bytes as usize,
                    }
                }),
        )
    }

    fn arb_agent_interrupt_result() -> OutputDocumentStrategy {
        serialized_output(
            (any::<bool>(), arb_small_string()).prop_map(|(interrupted, agent)| {
                crate::model::text::action_result::AgentInterruptResult { interrupted, agent }
            }),
        )
    }

    fn arb_agent_resume_result() -> OutputDocumentStrategy {
        serialized_output(
            (any::<bool>(), arb_small_string()).prop_map(|(resumed, agent)| {
                crate::model::text::action_result::AgentResumeResult { resumed, agent }
            }),
        )
    }

    fn arb_agent_simulate_crash_result() -> OutputDocumentStrategy {
        serialized_output(
            (any::<bool>(), arb_small_string()).prop_map(|(simulated, agent)| {
                crate::model::text::action_result::AgentSimulateCrashResult { simulated, agent }
            }),
        )
    }

    fn arb_account_delete_result() -> OutputDocumentStrategy {
        serialized_output(
            (any::<bool>(), arb_small_string()).prop_map(|(deleted, account_id)| {
                crate::model::text::account::AccountDeleteResult {
                    deleted,
                    account_id: golem_common::model::account::AccountId(
                        uuid::Uuid::parse_str(&account_id).expect("generated UUID should parse"),
                    ),
                }
            }),
        )
    }

    fn arb_account_get_result() -> OutputDocumentStrategy {
        serialized_output(arb_account().prop_map(crate::model::text::account::AccountGetView))
    }

    fn arb_account_new_result() -> OutputDocumentStrategy {
        serialized_output(arb_account().prop_map(crate::model::text::account::AccountNewView))
    }

    fn arb_account_update_result() -> OutputDocumentStrategy {
        serialized_output(arb_account().prop_map(crate::model::text::account::AccountUpdateView))
    }

    fn arb_account() -> BoxedStrategy<golem_client::model::Account> {
        (
            arb_uuid(),
            arb_small_u64(),
            arb_small_string(),
            arb_small_string(),
            arb_uuid(),
            proptest::collection::vec(arb_account_role(), 0..4),
            arb_uuid(),
        )
            .prop_map(
                |(id, revision, name, email, plan_id, roles, account_root_card_id)| {
                    golem_client::model::Account {
                        id: golem_common::model::account::AccountId(id),
                        revision: golem_common::model::account::AccountRevision::new(revision)
                            .expect("generated revision should be valid"),
                        name,
                        email: golem_common::model::account::AccountEmail::new(email),
                        plan_id: golem_common::model::plan::PlanId(plan_id),
                        roles,
                        account_root_card_id: golem_common::model::card::CardId(
                            account_root_card_id,
                        ),
                    }
                },
            )
            .boxed()
    }

    fn arb_account_role() -> BoxedStrategy<golem_common::model::auth::AccountRole> {
        prop_oneof![
            Just(golem_common::model::auth::AccountRole::Admin),
            Just(golem_common::model::auth::AccountRole::MarketingAdmin),
            Just(golem_common::model::auth::AccountRole::BuiltinPluginOwner),
        ]
        .boxed()
    }

    fn arb_permission_share_delete_result() -> OutputDocumentStrategy {
        serialized_output((any::<bool>(), arb_small_string()).prop_map(
            |(deleted, permission_share_id)| {
                crate::model::text::account::PermissionShareDeleteResult {
                    deleted,
                    permission_share_id: golem_common::model::permission_share::PermissionShareId(
                        uuid::Uuid::parse_str(&permission_share_id)
                            .expect("generated UUID should parse"),
                    ),
                }
            },
        ))
    }

    fn arb_permission_share_get_result() -> OutputDocumentStrategy {
        serialized_output(
            arb_permission_share().prop_map(crate::model::text::account::PermissionShareGetView),
        )
    }

    fn arb_permission_share_new_result() -> OutputDocumentStrategy {
        serialized_output(
            arb_permission_share().prop_map(crate::model::text::account::PermissionShareNewView),
        )
    }

    fn arb_permission_share_update_result() -> OutputDocumentStrategy {
        serialized_output(
            arb_permission_share().prop_map(crate::model::text::account::PermissionShareUpdateView),
        )
    }

    fn arb_permission_share_list_result() -> OutputDocumentStrategy {
        serialized_output(
            proptest::collection::vec(arb_permission_share(), 0..5).prop_map(|permission_shares| {
                crate::model::text::account::PermissionShareListView { permission_shares }
            }),
        )
    }

    fn arb_permission_share() -> BoxedStrategy<golem_client::model::PermissionShare> {
        (
            arb_uuid(),
            arb_small_u64(),
            arb_uuid(),
            arb_uuid(),
            arb_small_string(),
            proptest::option::of(arb_uuid()),
            arb_permission_share_data(),
        )
            .prop_map(
                |(
                    id,
                    revision,
                    owner_account_id,
                    target_account_id,
                    name,
                    current_card_id,
                    data,
                )| {
                    golem_client::model::PermissionShare {
                        id: golem_common::model::permission_share::PermissionShareId(id),
                        revision:
                            golem_common::model::permission_share::PermissionShareRevision::new(
                                revision,
                            )
                            .expect("generated revision should be valid"),
                        owner_account_id: golem_common::model::account::AccountId(owner_account_id),
                        target_account_id: golem_common::model::account::AccountId(
                            target_account_id,
                        ),
                        name: golem_common::model::permission_share::PermissionShareName(name),
                        current_card_id: current_card_id.map(golem_common::model::card::CardId),
                        data,
                    }
                },
            )
            .boxed()
    }

    fn arb_permission_share_data()
    -> BoxedStrategy<golem_common::model::permission_share::PermissionShareData> {
        (
            proptest::collection::vec(arb_small_string(), 0..4),
            proptest::collection::vec(arb_small_string(), 0..4),
            proptest::collection::vec(arb_small_string(), 0..4),
            proptest::collection::vec(arb_small_string(), 0..4),
        )
            .prop_map(
                |(lower_positive, lower_negative, upper_positive, upper_negative)| {
                    golem_common::model::permission_share::PermissionShareData {
                        lower_positive,
                        lower_negative,
                        upper_positive,
                        upper_negative,
                    }
                },
            )
            .boxed()
    }

    fn arb_agent_cancel_invocation_result() -> OutputDocumentStrategy {
        serialized_output(
            (any::<bool>(), arb_small_string(), arb_small_string()).prop_map(
                |(canceled, agent, idempotency_key)| {
                    crate::model::text::action_result::AgentCancelInvocationResult {
                        canceled,
                        agent,
                        idempotency_key,
                    }
                },
            ),
        )
    }

    fn arb_agent_redeploy_result() -> OutputDocumentStrategy {
        serialized_output(
            (
                any::<bool>(),
                proptest::collection::vec(arb_small_string(), 0..5),
            )
                .prop_map(|(redeployed, components)| {
                    crate::model::text::action_result::AgentRedeployResult {
                        redeployed,
                        components: components
                            .into_iter()
                            .map(golem_common::model::component::ComponentName)
                            .collect(),
                    }
                }),
        )
    }

    fn arb_agent_revert_result() -> OutputDocumentStrategy {
        serialized_output(
            (
                any::<bool>(),
                arb_small_string(),
                proptest::option::of(arb_small_u64()),
                proptest::option::of(arb_small_u64()),
            )
                .prop_map(
                    |(reverted, agent, last_oplog_index, number_of_invocations)| {
                        crate::model::text::action_result::AgentRevertResult {
                            reverted,
                            agent,
                            last_oplog_index,
                            number_of_invocations,
                        }
                    },
                ),
        )
    }

    fn arb_agent_plugin_toggle_result() -> OutputDocumentStrategy {
        serialized_output(
            (
                any::<bool>(),
                arb_small_string(),
                arb_small_string(),
                0i32..1000,
            )
                .prop_map(|(activated, agent, plugin, priority)| {
                    crate::model::text::action_result::AgentPluginToggleResult {
                        activated,
                        agent,
                        plugin,
                        priority,
                    }
                }),
        )
    }

    fn arb_token_delete_result() -> OutputDocumentStrategy {
        serialized_output(
            (any::<bool>(), arb_small_string()).prop_map(|(deleted, token_id)| {
                crate::model::text::token::TokenDeleteResult {
                    deleted,
                    token_id: golem_common::model::auth::TokenId(
                        uuid::Uuid::parse_str(&token_id).expect("generated UUID should parse"),
                    ),
                }
            }),
        )
    }

    fn arb_token_list_result() -> OutputDocumentStrategy {
        serialized_output(
            proptest::collection::vec(arb_token(), 0..5)
                .prop_map(|tokens| crate::model::text::token::TokenListView { tokens }),
        )
    }

    fn arb_token_new_result() -> OutputDocumentStrategy {
        serialized_output(arb_token_with_secret().prop_map(crate::model::text::token::TokenNewView))
    }

    fn arb_token() -> BoxedStrategy<golem_common::model::auth::Token> {
        (arb_uuid(), arb_uuid(), arb_datetime(), arb_datetime())
            .prop_map(
                |(id, account_id, created_at, expires_at)| golem_common::model::auth::Token {
                    id: golem_common::model::auth::TokenId(id),
                    account_id: golem_common::model::account::AccountId(account_id),
                    created_at,
                    expires_at,
                },
            )
            .boxed()
    }

    fn arb_token_with_secret() -> BoxedStrategy<golem_common::model::auth::TokenWithSecret> {
        (
            arb_uuid(),
            arb_uuid(),
            arb_small_string(),
            arb_datetime(),
            arb_datetime(),
        )
            .prop_map(|(id, account_id, secret, created_at, expires_at)| {
                golem_common::model::auth::TokenWithSecret {
                    id: golem_common::model::auth::TokenId(id),
                    secret: golem_common::model::auth::TokenSecret::trusted(secret),
                    account_id: golem_common::model::account::AccountId(account_id),
                    created_at,
                    expires_at,
                }
            })
            .boxed()
    }

    fn arb_api_domain_delete_result() -> OutputDocumentStrategy {
        serialized_output(
            (any::<bool>(), arb_small_string(), arb_small_string()).prop_map(
                |(deleted, domain, id)| {
                    crate::model::text::http_api_domain::DomainRegistrationDeleteResult {
                        deleted,
                        domain: golem_common::model::domain_registration::Domain(domain),
                        id: golem_common::model::domain_registration::DomainRegistrationId(
                            uuid::Uuid::parse_str(&id).expect("generated UUID should parse"),
                        ),
                    }
                },
            ),
        )
    }

    fn arb_api_domain_register_result() -> OutputDocumentStrategy {
        serialized_output(
            arb_domain_registration()
                .prop_map(crate::model::text::http_api_domain::DomainRegistrationNewView),
        )
    }

    fn arb_api_domain_list_result() -> OutputDocumentStrategy {
        serialized_output(
            proptest::collection::vec(arb_domain_registration(), 0..5).prop_map(|domains| {
                crate::model::text::http_api_domain::HttpApiDomainListView { domains }
            }),
        )
    }

    fn arb_domain_registration()
    -> BoxedStrategy<golem_common::model::domain_registration::DomainRegistration> {
        (arb_uuid(), arb_uuid(), arb_small_string())
            .prop_map(|(id, environment_id, domain)| {
                golem_common::model::domain_registration::DomainRegistration {
                    id: golem_common::model::domain_registration::DomainRegistrationId(id),
                    environment_id: golem_common::model::environment::EnvironmentId(environment_id),
                    domain: golem_common::model::domain_registration::Domain(domain),
                }
            })
            .boxed()
    }

    fn arb_api_deployment_get_result() -> OutputDocumentStrategy {
        serialized_output(
            arb_http_api_deployment()
                .prop_map(crate::model::text::http_api_deployment::HttpApiDeploymentGetView),
        )
    }

    fn arb_api_deployment_list_result() -> OutputDocumentStrategy {
        serialized_output(
            proptest::collection::vec(arb_http_api_deployment(), 0..5).prop_map(|deployments| {
                crate::model::text::http_api_deployment::HttpApiDeploymentListView { deployments }
            }),
        )
    }

    fn arb_http_api_deployment() -> BoxedStrategy<golem_client::model::HttpApiDeployment> {
        (
            arb_uuid(),
            arb_small_u64(),
            arb_uuid(),
            arb_small_string(),
            proptest::collection::btree_map(
                arb_agent_type_name(),
                arb_http_api_deployment_agent_options(),
                0..4,
            ),
            arb_small_string(),
            arb_small_string(),
            arb_datetime(),
        )
            .prop_map(|(id, revision, environment_id, domain, agents, webhooks_prefix, openapi_endpoint_prefix, created_at)| {
                golem_client::model::HttpApiDeployment {
                    id: golem_common::model::http_api_deployment::HttpApiDeploymentId(id),
                    revision: golem_common::model::http_api_deployment::HttpApiDeploymentRevision::new(revision)
                        .expect("generated revision should be valid"),
                    environment_id: golem_common::model::environment::EnvironmentId(environment_id),
                    domain: golem_common::model::domain_registration::Domain(domain.clone()),
                    hash: golem_common::model::diff::Hash::new(blake3::hash(domain.as_bytes())),
                    agents,
                    webhooks_prefix,
                    openapi_endpoint_prefix,
                    created_at,
                }
            })
            .boxed()
    }

    fn arb_agent_type_name() -> BoxedStrategy<golem_common::model::agent::AgentTypeName> {
        arb_small_string()
            .prop_map(golem_common::model::agent::AgentTypeName)
            .boxed()
    }

    fn arb_http_api_deployment_agent_options()
    -> BoxedStrategy<golem_common::model::http_api_deployment::HttpApiDeploymentAgentOptions> {
        proptest::option::of(prop_oneof![
            arb_small_string().prop_map(|header_name| {
                golem_common::model::http_api_deployment::HttpApiDeploymentAgentSecurity::TestSessionHeader(
                    golem_common::model::http_api_deployment::TestSessionHeaderAgentSecurity { header_name },
                )
            }),
            arb_small_string().prop_map(|security_scheme| {
                golem_common::model::http_api_deployment::HttpApiDeploymentAgentSecurity::SecurityScheme(
                    golem_common::model::http_api_deployment::SecuritySchemeAgentSecurity {
                        security_scheme: golem_common::model::security_scheme::SecuritySchemeName(security_scheme),
                    },
                )
            }),
        ])
        .prop_map(|security| {
            golem_common::model::http_api_deployment::HttpApiDeploymentAgentOptions {
                security,
            }
        })
        .boxed()
    }

    fn arb_api_security_scheme_create_result() -> OutputDocumentStrategy {
        serialized_output(
            arb_security_scheme()
                .prop_map(crate::model::text::http_api_security::HttpSecuritySchemeCreateView),
        )
    }

    fn arb_api_security_scheme_delete_result() -> OutputDocumentStrategy {
        serialized_output(
            arb_security_scheme()
                .prop_map(crate::model::text::http_api_security::HttpSecuritySchemeDeleteView),
        )
    }

    fn arb_api_security_scheme_get_result() -> OutputDocumentStrategy {
        serialized_output(
            arb_security_scheme()
                .prop_map(crate::model::text::http_api_security::HttpSecuritySchemeGetView),
        )
    }

    fn arb_api_security_scheme_update_result() -> OutputDocumentStrategy {
        serialized_output(
            arb_security_scheme()
                .prop_map(crate::model::text::http_api_security::HttpSecuritySchemeUpdateView),
        )
    }

    fn arb_api_security_scheme_list_result() -> OutputDocumentStrategy {
        serialized_output(
            proptest::collection::vec(arb_security_scheme(), 0..5).prop_map(|security_schemes| {
                crate::model::text::http_api_security::HttpSecuritySchemeListView {
                    security_schemes,
                }
            }),
        )
    }

    fn arb_security_scheme() -> BoxedStrategy<golem_client::model::SecuritySchemeDto> {
        (
            arb_uuid(),
            arb_small_u64(),
            arb_small_string(),
            arb_uuid(),
            arb_security_scheme_provider(),
            arb_small_string(),
            arb_url_string(),
            proptest::collection::vec(arb_small_string(), 0..5),
        )
            .prop_map(
                |(
                    id,
                    revision,
                    name,
                    environment_id,
                    provider_type,
                    client_id,
                    redirect_url,
                    scopes,
                )| {
                    golem_client::model::SecuritySchemeDto {
                        id: golem_common::model::security_scheme::SecuritySchemeId(id),
                        revision:
                            golem_common::model::security_scheme::SecuritySchemeRevision::new(
                                revision,
                            )
                            .expect("generated revision should be valid"),
                        name: golem_common::model::security_scheme::SecuritySchemeName(name),
                        environment_id: golem_common::model::environment::EnvironmentId(
                            environment_id,
                        ),
                        provider_type,
                        client_id,
                        redirect_url,
                        scopes,
                    }
                },
            )
            .boxed()
    }

    fn arb_security_scheme_provider()
    -> BoxedStrategy<golem_common::model::security_scheme::Provider> {
        prop_oneof![
            Just(golem_common::model::security_scheme::Provider::Google(
                golem_common::model::Empty {}
            )),
            Just(golem_common::model::security_scheme::Provider::Facebook(
                golem_common::model::Empty {}
            )),
            Just(golem_common::model::security_scheme::Provider::Microsoft(
                golem_common::model::Empty {}
            )),
            Just(golem_common::model::security_scheme::Provider::Gitlab(
                golem_common::model::Empty {}
            )),
            (arb_small_string(), arb_url_string()).prop_map(|(name, issuer_url)| {
                golem_common::model::security_scheme::Provider::Custom(
                    golem_common::model::security_scheme::CustomProvider { name, issuer_url },
                )
            }),
        ]
        .boxed()
    }

    fn arb_new_app_result() -> OutputDocumentStrategy {
        serialized_output(
            (any::<bool>(), arb_small_string(), arb_small_string()).prop_map(
                |(created, application_name, application_dir)| {
                    crate::model::text::action_result::NewAppResult {
                        created,
                        application_name,
                        application_dir: PathBuf::from(application_dir),
                    }
                },
            ),
        )
    }

    fn arb_template_list_result() -> OutputDocumentStrategy {
        serialized_output(
            proptest::collection::vec(arb_template_description(), 0..5)
                .prop_map(|templates| crate::model::text::template::TemplateListView { templates }),
        )
    }

    fn arb_template_description() -> BoxedStrategy<crate::model::TemplateDescription> {
        (arb_small_string(), arb_guest_language(), arb_small_string())
            .prop_map(
                |(name, language, description)| crate::model::TemplateDescription {
                    name,
                    language,
                    description,
                },
            )
            .boxed()
    }

    fn arb_guest_language() -> BoxedStrategy<crate::model::GuestLanguage> {
        prop_oneof![
            Just(crate::model::GuestLanguage::TypeScript),
            Just(crate::model::GuestLanguage::Rust),
            Just(crate::model::GuestLanguage::Scala),
            Just(crate::model::GuestLanguage::MoonBit)
        ]
        .boxed()
    }

    fn arb_component_get_result() -> OutputDocumentStrategy {
        serialized_output(
            arb_component_view().prop_map(crate::model::text::component::ComponentGetView),
        )
    }

    fn arb_component_list_result() -> OutputDocumentStrategy {
        serialized_output(
            proptest::collection::vec(arb_component_view(), 0..5).prop_map(|components| {
                crate::model::text::component::ComponentListView { components }
            }),
        )
    }

    fn arb_component_manifest_trace_result() -> OutputDocumentStrategy {
        serialized_output(
            (arb_small_string(), arb_component_layer_properties()).prop_map(
                |(component_name, properties)| {
                    crate::model::text::component::ComponentManifestTraceView {
                        component_name: golem_common::model::component::ComponentName(
                            component_name,
                        ),
                        properties,
                    }
                },
            ),
        )
    }

    fn arb_component_layer_properties() -> BoxedStrategy<crate::model::app::ComponentLayerProperties>
    {
        (
            (
                arb_component_layer_id(),
                arb_component_layer_id(),
                proptest::option::of(arb_small_string()),
                proptest::option::of(arb_small_string()),
                proptest::option::of(arb_small_string()),
                arb_vec_merge_mode(),
                proptest::collection::vec(arb_manifest_build_command(), 1..3),
                arb_map_merge_mode(),
                proptest::collection::hash_map(
                    arb_small_string(),
                    proptest::collection::vec(arb_manifest_external_command(), 1..3),
                    1..3,
                ),
            ),
            (
                arb_vec_merge_mode(),
                proptest::collection::vec(arb_small_string(), 1..3),
                arb_json_value(1),
                arb_json_value(1),
                arb_map_merge_mode(),
                proptest::collection::hash_map(arb_small_string(), arb_small_string(), 1..3),
                arb_vec_merge_mode(),
                proptest::collection::vec(arb_manifest_plugin_installation(), 1..3),
                arb_vec_merge_mode(),
                proptest::collection::vec(arb_manifest_initial_component_file(), 1..3),
            ),
        )
            .prop_map(|(left, right)| {
                use crate::model::cascade::property::Property;

                let (
                    layer_id,
                    second_layer_id,
                    selection,
                    component_wasm,
                    output_wasm,
                    _build_mode,
                    build,
                    _custom_commands_mode,
                    custom_commands,
                ) = left;
                let (
                    _clean_mode,
                    clean,
                    config_first,
                    config_second,
                    _env_mode,
                    env,
                    _plugins_mode,
                    plugins,
                    _files_mode,
                    files,
                ) = right;

                let mut properties = crate::model::app::ComponentLayerProperties::default();
                let selection = selection.as_ref();

                properties
                    .component_wasm
                    .apply_layer(&layer_id, selection, None);
                properties
                    .component_wasm
                    .apply_layer(&second_layer_id, selection, component_wasm);
                properties
                    .output_wasm
                    .apply_layer(&second_layer_id, selection, output_wasm);
                properties.build.apply_layer(
                    &layer_id,
                    selection,
                    (
                        crate::model::cascade::property::vec::VecMergeMode::Append,
                        build,
                    ),
                );
                properties.clean.apply_layer(
                    &layer_id,
                    selection,
                    (
                        crate::model::cascade::property::vec::VecMergeMode::Prepend,
                        clean,
                    ),
                );
                properties.custom_commands.apply_layer(
                    &layer_id,
                    selection,
                    (
                        crate::model::cascade::property::map::MapMergeMode::Upsert,
                        custom_commands.clone().into_iter().collect(),
                    ),
                );
                properties.custom_commands.apply_layer(
                    &second_layer_id,
                    selection,
                    (
                        crate::model::cascade::property::map::MapMergeMode::Replace,
                        custom_commands.clone().into_iter().collect(),
                    ),
                );
                properties.custom_commands.apply_layer(
                    &layer_id,
                    selection,
                    (
                        crate::model::cascade::property::map::MapMergeMode::Remove,
                        custom_commands.into_iter().collect(),
                    ),
                );
                properties
                    .config
                    .apply_layer(&layer_id, selection, Some(config_first));
                properties
                    .config
                    .apply_layer(&second_layer_id, selection, Some(config_second));
                properties
                    .config
                    .apply_layer(&layer_id, selection, Some(json!("replacement")));
                properties.env.apply_layer(
                    &layer_id,
                    selection,
                    (
                        crate::model::cascade::property::map::MapMergeMode::Upsert,
                        env.clone().into_iter().collect(),
                    ),
                );
                properties.env.apply_layer(
                    &layer_id,
                    selection,
                    (
                        crate::model::cascade::property::map::MapMergeMode::Remove,
                        env.into_iter().collect(),
                    ),
                );
                properties.plugins.apply_layer(
                    &layer_id,
                    selection,
                    (
                        crate::model::cascade::property::vec::VecMergeMode::Replace,
                        plugins,
                    ),
                );
                properties.files.apply_layer(
                    &layer_id,
                    selection,
                    (
                        crate::model::cascade::property::vec::VecMergeMode::Append,
                        files.clone(),
                    ),
                );
                properties.files.apply_layer(
                    &second_layer_id,
                    selection,
                    (
                        crate::model::cascade::property::vec::VecMergeMode::Replace,
                        files,
                    ),
                );

                properties
            })
            .boxed()
    }

    fn arb_component_layer_id() -> BoxedStrategy<crate::model::app::ComponentLayerId> {
        prop_oneof![
            arb_small_string().prop_map(crate::model::app::ComponentLayerId::TemplateCommon),
            arb_small_string()
                .prop_map(crate::model::app::ComponentLayerId::TemplateEnvironmentPresets),
            arb_small_string().prop_map(crate::model::app::ComponentLayerId::TemplateCustomPresets),
            arb_small_string().prop_map(|name| {
                crate::model::app::ComponentLayerId::ComponentCommon(
                    golem_common::model::component::ComponentName(name),
                )
            }),
            arb_small_string().prop_map(|name| {
                crate::model::app::ComponentLayerId::ComponentEnvironmentPresets(
                    golem_common::model::component::ComponentName(name),
                )
            }),
            arb_small_string().prop_map(|name| {
                crate::model::app::ComponentLayerId::ComponentCustomPresets(
                    golem_common::model::component::ComponentName(name),
                )
            }),
        ]
        .boxed()
    }

    fn arb_vec_merge_mode() -> BoxedStrategy<crate::model::cascade::property::vec::VecMergeMode> {
        prop_oneof![
            Just(crate::model::cascade::property::vec::VecMergeMode::Append),
            Just(crate::model::cascade::property::vec::VecMergeMode::Prepend),
            Just(crate::model::cascade::property::vec::VecMergeMode::Replace),
        ]
        .boxed()
    }

    fn arb_map_merge_mode() -> BoxedStrategy<crate::model::cascade::property::map::MapMergeMode> {
        prop_oneof![
            Just(crate::model::cascade::property::map::MapMergeMode::Upsert),
            Just(crate::model::cascade::property::map::MapMergeMode::Replace),
            Just(crate::model::cascade::property::map::MapMergeMode::Remove),
        ]
        .boxed()
    }

    fn arb_manifest_build_command() -> BoxedStrategy<crate::model::app_raw::BuildCommand> {
        prop_oneof![
            arb_manifest_external_command().prop_map(crate::model::app_raw::BuildCommand::External),
            (
                arb_small_string(),
                arb_small_string(),
                proptest::collection::hash_map(arb_small_string(), arb_small_string(), 0..3),
                proptest::option::of(arb_small_string()),
            )
                .prop_map(|(generate_quickjs_crate, wit, js_modules, world)| {
                    crate::model::app_raw::BuildCommand::QuickJSCrate(
                        crate::model::app_raw::GenerateQuickJSCrate {
                            generate_quickjs_crate,
                            wit,
                            js_modules,
                            world,
                        },
                    )
                }),
            (
                arb_small_string(),
                arb_small_string(),
                proptest::option::of(arb_small_string())
            )
                .prop_map(|(generate_quickjs_dts, wit, world)| {
                    crate::model::app_raw::BuildCommand::QuickJSDTS(
                        crate::model::app_raw::GenerateQuickJSDTS {
                            generate_quickjs_dts,
                            wit,
                            world,
                        },
                    )
                }),
            (arb_small_string(), arb_small_string(), arb_small_string()).prop_map(
                |(inject_to_prebuilt_quickjs, module, into)| {
                    crate::model::app_raw::BuildCommand::InjectToPrebuiltQuickJs(
                        crate::model::app_raw::InjectToPrebuiltQuickJs {
                            inject_to_prebuilt_quickjs,
                            module,
                            into,
                        },
                    )
                }
            ),
            (arb_small_string(), arb_small_string()).prop_map(|(preinitialize_js, into)| {
                crate::model::app_raw::BuildCommand::PreinitializeJs(
                    crate::model::app_raw::PreinitializeJs {
                        preinitialize_js,
                        into,
                    },
                )
            }),
        ]
        .boxed()
    }

    fn arb_manifest_external_command() -> BoxedStrategy<crate::model::app_raw::ExternalCommand> {
        (
            arb_small_string(),
            proptest::option::of(arb_small_string()),
            proptest::collection::hash_map(arb_small_string(), arb_small_string(), 0..3),
            proptest::collection::vec(arb_small_string(), 0..3),
            proptest::collection::vec(arb_small_string(), 0..3),
            proptest::collection::vec(arb_small_string(), 0..3),
            proptest::collection::vec(arb_small_string(), 0..3),
        )
            .prop_map(|(command, dir, env, rmdirs, mkdirs, sources, targets)| {
                crate::model::app_raw::ExternalCommand {
                    command,
                    dir,
                    env: env.into_iter().collect(),
                    rmdirs,
                    mkdirs,
                    sources,
                    targets,
                }
            })
            .boxed()
    }

    fn arb_manifest_plugin_installation() -> BoxedStrategy<crate::model::app_raw::PluginInstallation>
    {
        (
            proptest::option::of(arb_small_string()),
            arb_small_string(),
            arb_small_string(),
            proptest::collection::hash_map(arb_small_string(), arb_small_string(), 0..3),
        )
            .prop_map(|(account, name, version, parameters)| {
                crate::model::app_raw::PluginInstallation {
                    account,
                    name,
                    version,
                    parameters,
                }
            })
            .boxed()
    }

    fn arb_manifest_initial_component_file()
    -> BoxedStrategy<crate::model::app_raw::InitialComponentFile> {
        (
            arb_small_string(),
            arb_small_string(),
            proptest::option::of(arb_agent_file_permissions()),
        )
            .prop_map(|(source_path, target_path, permissions)| {
                crate::model::app_raw::InitialComponentFile {
                    source_path,
                    target_path: golem_common::model::component::CanonicalFilePath::from_abs_str(
                        &format!("/{target_path}"),
                    )
                    .expect("generated path should be valid"),
                    permissions,
                }
            })
            .boxed()
    }

    fn arb_component_view() -> BoxedStrategy<crate::model::component::ComponentView> {
        (
            arb_small_string(),
            arb_uuid(),
            proptest::option::of(arb_small_string()),
            arb_small_u64(),
            arb_small_u64(),
            Just(fixed_datetime()),
            arb_uuid(),
            proptest::collection::vec(arb_small_string(), 0..5),
            proptest::collection::vec(arb_agent_type(), 0..3),
            proptest::collection::btree_map(
                arb_agent_type_name(),
                arb_agent_type_provision_config(),
                0..3,
            ),
        )
            .prop_map(
                |(
                    component_name,
                    component_id,
                    component_version,
                    component_revision,
                    component_size,
                    created_at,
                    environment_id,
                    exports,
                    agent_types,
                    agent_type_provision_configs,
                )| {
                    crate::model::component::ComponentView {
                        component_name: golem_common::model::component::ComponentName(
                            component_name,
                        ),
                        component_id: golem_common::model::component::ComponentId(component_id),
                        component_version,
                        component_revision,
                        component_size,
                        created_at,
                        environment_id: golem_common::model::environment::EnvironmentId(
                            environment_id,
                        ),
                        exports,
                        agent_types,
                        agent_type_provision_configs,
                    }
                },
            )
            .boxed()
    }

    fn arb_agent_type_provision_config()
    -> BoxedStrategy<golem_common::model::component_metadata::AgentTypeProvisionConfig> {
        (
            proptest::collection::btree_map(arb_small_string(), arb_small_string(), 0..3),
            proptest::collection::vec(arb_typed_agent_config_entry(), 0..3),
            proptest::collection::vec(arb_installed_plugin(), 0..2),
            proptest::collection::vec(arb_initial_agent_file(), 0..2),
        )
            .prop_map(|(env, config, plugins, files)| {
                golem_common::model::component_metadata::AgentTypeProvisionConfig {
                    env,
                    config,
                    plugins,
                    files,
                }
            })
            .boxed()
    }

    fn arb_typed_agent_config_entry()
    -> BoxedStrategy<golem_common::model::worker::TypedAgentConfigEntry> {
        (
            proptest::collection::vec(arb_small_string(), 1..3),
            arb_small_string(),
        )
            .prop_map(
                |(path, value)| golem_common::model::worker::TypedAgentConfigEntry {
                    path,
                    value: golem_common::schema::TypedSchemaValue::new(
                        golem_common::schema::SchemaGraph::anonymous(
                            golem_common::schema::SchemaType::string(),
                        ),
                        golem_common::schema::SchemaValue::String(value),
                    ),
                },
            )
            .boxed()
    }

    fn arb_installed_plugin() -> BoxedStrategy<golem_common::model::component::InstalledPlugin> {
        (
            arb_uuid(),
            0i32..1000,
            proptest::collection::btree_map(arb_small_string(), arb_small_string(), 0..3),
            arb_uuid(),
            arb_small_string(),
            arb_small_string(),
            proptest::option::of(arb_uuid()),
            proptest::option::of(arb_small_u64()),
        )
            .prop_map(
                |(
                    environment_plugin_grant_id,
                    priority,
                    parameters,
                    plugin_registration_id,
                    plugin_name,
                    plugin_version,
                    oplog_processor_component_id,
                    oplog_processor_component_revision,
                )| {
                    golem_common::model::component::InstalledPlugin {
                        environment_plugin_grant_id:
                            golem_common::model::environment_plugin_grant::EnvironmentPluginGrantId(
                                environment_plugin_grant_id,
                            ),
                        priority: golem_common::model::component::PluginPriority(priority),
                        parameters,
                        plugin_registration_id:
                            golem_common::model::plugin_registration::PluginRegistrationId(
                                plugin_registration_id,
                            ),
                        plugin_name,
                        plugin_version,
                        oplog_processor_component_id: oplog_processor_component_id
                            .map(golem_common::model::component::ComponentId),
                        oplog_processor_component_revision: oplog_processor_component_revision.map(
                            |revision| {
                                golem_common::model::component::ComponentRevision::new(revision)
                                    .expect("generated revision should be valid")
                            },
                        ),
                    }
                },
            )
            .boxed()
    }

    fn arb_initial_agent_file() -> BoxedStrategy<golem_common::model::component::InitialAgentFile> {
        (
            arb_hash(),
            arb_small_string(),
            arb_agent_file_permissions(),
            arb_small_u64(),
        )
            .prop_map(|(content_hash, path, permissions, size)| {
                golem_common::model::component::InitialAgentFile {
                    content_hash: golem_common::model::agent::AgentFileContentHash(content_hash),
                    path: golem_common::model::component::AgentFilePath::from_abs_str(&format!(
                        "/{path}"
                    ))
                    .expect("generated path should be valid"),
                    permissions,
                    size,
                }
            })
            .boxed()
    }

    fn arb_agent_file_permissions()
    -> BoxedStrategy<golem_common::model::component::AgentFilePermissions> {
        prop_oneof![
            Just(golem_common::model::component::AgentFilePermissions::ReadOnly),
            Just(golem_common::model::component::AgentFilePermissions::ReadWrite),
        ]
        .boxed()
    }

    fn arb_deploy_plan_result() -> OutputDocumentStrategy {
        any::<bool>()
            .prop_map(|include_environment_setup| {
                let deployment_diff = empty_deployment_diff();
                let environment_setup = include_environment_setup
                    .then(crate::model::deploy::EnvironmentSetupPlan::default);
                to_structured_output_value_masked(
                    DeployPlanView {
                        deployment_diff: &deployment_diff,
                        environment_setup: environment_setup.as_ref(),
                    },
                    MaskingConfig::hide_secrets(),
                )
                .expect("generated deploy plan should serialize")
            })
            .boxed()
    }

    fn arb_deployment_diff_result() -> OutputDocumentStrategy {
        arb_deployment_diff()
            .prop_map(|diff| {
                to_structured_output_value_masked(diff, MaskingConfig::hide_secrets())
                    .expect("generated deployment diff should serialize")
            })
            .boxed()
    }

    fn arb_deployment_diff() -> BoxedStrategy<golem_common::model::diff::DeploymentDiff> {
        (
            arb_small_string(),
            arb_small_string(),
            arb_small_string(),
            arb_hash(),
            arb_hash(),
            arb_hash(),
            arb_hash(),
            arb_hash(),
            arb_hash(),
        )
            .prop_map(|(component_key, http_key, mcp_key, current_component_hash, new_component_hash, current_file_hash, new_file_hash, current_mcp_hash, new_mcp_hash)| {
                use golem_common::model::diff::Diffable;

                let mut current = golem_common::model::diff::Deployment::default();
                let mut new = golem_common::model::diff::Deployment::default();

                let current_component = golem_common::model::diff::Component {
                    wasm_hash: current_component_hash,
                    agent_type_provision_configs: BTreeMap::from_iter([(
                        "agent".to_string(),
                        golem_common::model::diff::HashOf::form_value(
                            golem_common::model::diff::AgentTypeProvisionConfig {
                                env: BTreeMap::from_iter([("A".to_string(), "old".to_string())]),
                                config: BTreeMap::from_iter([(
                                    "path".to_string(),
                                    golem_common::base_model::json::NormalizedJsonValue(json!("old")),
                                )]),
                                files_by_path: BTreeMap::from_iter([(
                                    "/config.json".to_string(),
                                    golem_common::model::diff::HashOf::form_value(
                                        golem_common::model::diff::AgentFile {
                                            hash: current_file_hash,
                                            permissions: golem_common::model::component::AgentFilePermissions::ReadOnly,
                                        },
                                    ),
                                )]),
                                plugins_by_grant_id: BTreeMap::new(),
                            },
                        ),
                    )]),
                };

                let new_component = golem_common::model::diff::Component {
                    wasm_hash: new_component_hash,
                    agent_type_provision_configs: BTreeMap::from_iter([(
                        "agent".to_string(),
                        golem_common::model::diff::HashOf::form_value(
                            golem_common::model::diff::AgentTypeProvisionConfig {
                                env: BTreeMap::from_iter([("A".to_string(), "new".to_string())]),
                                config: BTreeMap::from_iter([(
                                    "path".to_string(),
                                    golem_common::base_model::json::NormalizedJsonValue(json!("new")),
                                )]),
                                files_by_path: BTreeMap::from_iter([(
                                    "/config.json".to_string(),
                                    golem_common::model::diff::HashOf::form_value(
                                        golem_common::model::diff::AgentFile {
                                            hash: new_file_hash,
                                            permissions: golem_common::model::component::AgentFilePermissions::ReadWrite,
                                        },
                                    ),
                                )]),
                                plugins_by_grant_id: BTreeMap::new(),
                            },
                        ),
                    )]),
                };

                current.components.insert(
                    component_key.clone(),
                    golem_common::model::diff::HashOf::form_value(current_component),
                );
                new.components.insert(
                    component_key,
                    golem_common::model::diff::HashOf::form_value(new_component),
                );

                new.http_api_deployments.insert(
                    http_key.clone(),
                    golem_common::model::diff::HashOf::form_value(
                        golem_common::model::diff::HttpApiDeployment {
                            webhooks_prefix: "new-webhooks".to_string(),
                            openapi_endpoint_prefix: "new-openapi".to_string(),
                            agents: BTreeMap::from_iter([(
                                "agent".to_string(),
                                golem_common::model::diff::HttpApiDeploymentAgentOptions {
                                    security_scheme: Some("new-scheme".to_string()),
                                    test_session_header: Some("x-test".to_string()),
                                },
                            )]),
                        },
                    ),
                );
                current.http_api_deployments.insert(
                    http_key,
                    golem_common::model::diff::HashOf::form_value(
                        golem_common::model::diff::HttpApiDeployment {
                            webhooks_prefix: "old-webhooks".to_string(),
                            openapi_endpoint_prefix: "old-openapi".to_string(),
                            agents: BTreeMap::from_iter([(
                                "agent".to_string(),
                                golem_common::model::diff::HttpApiDeploymentAgentOptions {
                                    security_scheme: Some("old-scheme".to_string()),
                                    test_session_header: None,
                                },
                            )]),
                        },
                    ),
                );

                current.mcp_deployments.insert(
                    mcp_key.clone(),
                    golem_common::model::diff::HashOf::form_value(
                        golem_common::model::diff::McpDeployment {
                            agents: BTreeMap::from_iter([(
                                "agent".to_string(),
                                golem_common::model::diff::McpDeploymentAgentOptions {
                                    security_scheme: Some(current_mcp_hash.to_string()),
                                },
                            )]),
                        },
                    ),
                );
                new.mcp_deployments.insert(
                    mcp_key,
                    golem_common::model::diff::HashOf::form_value(
                        golem_common::model::diff::McpDeployment {
                            agents: BTreeMap::from_iter([(
                                "agent".to_string(),
                                golem_common::model::diff::McpDeploymentAgentOptions {
                                    security_scheme: Some(new_mcp_hash.to_string()),
                                },
                            )]),
                        },
                    ),
                );

                golem_common::model::diff::Deployment::diff(&new, &current)
                    .expect("generated deployments should diff")
                    .expect("generated deployments should differ")
            })
            .boxed()
    }

    fn arb_environment_setup_plan_result() -> OutputDocumentStrategy {
        arb_environment_setup_plan()
            .prop_map(|output| {
                to_structured_output_value(crate::model::text::diff::EnvironmentSetupPlanView(
                    &output,
                ))
                .expect("generated environment setup plan should serialize")
            })
            .boxed()
    }

    fn arb_environment_setup_plan() -> BoxedStrategy<crate::model::deploy::EnvironmentSetupPlan> {
        (
            arb_small_string(),
            arb_api_predicate(),
            arb_api_retry_policy(),
            arb_resource_limit(),
            arb_enforcement_action(),
        )
            .prop_map(|(name, predicate, policy, limit, enforcement_action)| {
                let secret_path = golem_common::model::agent_secret::AgentSecretPath(vec![
                    name.clone(),
                    "token".to_string(),
                ]);
                let retry_policy_default =
                    golem_common::model::deployment::DeploymentRetryPolicyDefault {
                        name: name.clone(),
                        priority: 1,
                        predicate: predicate.clone(),
                        policy: policy.clone(),
                    };
                let resource_default = golem_common::model::quota::ResourceDefinitionCreation {
                    name: golem_common::model::quota::ResourceName(name.clone()),
                    limit: limit.clone(),
                    enforcement_action,
                    unit: "request".to_string(),
                    units: "requests".to_string(),
                };

                crate::model::deploy::EnvironmentSetupPlan {
                    display: crate::model::deploy::EnvironmentSetupDisplay {
                        to_be_applied: crate::model::deploy::EnvironmentSetupDetailedSection {
                            secret_values: BTreeMap::from_iter([(
                                name.clone(),
                                crate::model::deploy::EnvironmentSetupSecretValueDisplay {
                                    secret_type: "Str".to_string(),
                                    value: json!("generated-secret"),
                                },
                            )]),
                            retry_policies: BTreeMap::from_iter([(
                                name.clone(),
                                crate::model::deploy::EnvironmentSetupRetryPolicyDisplay {
                                    priority: retry_policy_default.priority,
                                    predicate: serde_json::to_value(&predicate)
                                        .expect("generated predicate should serialize"),
                                    policy: serde_json::to_value(&policy)
                                        .expect("generated policy should serialize"),
                                },
                            )]),
                            resources: BTreeMap::from_iter([(
                                name.clone(),
                                crate::model::deploy::EnvironmentSetupResourceDisplay {
                                    limit: serde_json::to_value(&limit)
                                        .expect("generated limit should serialize"),
                                    enforcement_action: format!("{enforcement_action:?}"),
                                    unit: "request".to_string(),
                                    units: "requests".to_string(),
                                },
                            )]),
                        },
                        skipped_already_exists:
                            crate::model::deploy::EnvironmentSetupKeysOnlySection {
                                secret_values: BTreeSet::from_iter([format!("{name}-existing")]),
                                retry_policies: BTreeSet::from_iter([format!("{name}-retry")]),
                                resources: BTreeSet::from_iter([format!("{name}-resource")]),
                            },
                    },
                    agent_secret_defaults: vec![
                        golem_common::model::deployment::DeploymentAgentSecretDefault {
                            path: secret_path.clone(),
                            secret_value: json!("generated-secret"),
                        },
                    ],
                    skipped_existing_agent_secret_defaults: vec![
                        golem_common::model::deployment::DeploymentAgentSecretDefault {
                            path: secret_path,
                            secret_value: json!("existing-secret"),
                        },
                    ],
                    retry_policy_defaults: vec![retry_policy_default],
                    resource_defaults: vec![resource_default],
                }
            })
            .boxed()
    }

    fn arb_deployment_create_result() -> OutputDocumentStrategy {
        serialized_output(
            (
                arb_small_string(),
                arb_small_string(),
                arb_current_deployment(),
            )
                .prop_map(|(application_name, environment_name, deployment)| {
                    crate::model::text::deployment::DeploymentNewView {
                        application_name: golem_common::model::application::ApplicationName(
                            application_name,
                        ),
                        environment_name: golem_common::model::environment::EnvironmentName(
                            environment_name,
                        ),
                        deployment,
                    }
                }),
        )
    }

    fn arb_deployment_list_result() -> OutputDocumentStrategy {
        serialized_output(proptest::collection::vec(arb_deployment(), 0..5).prop_map(
            |deployments| crate::model::text::deployment::DeploymentListView { deployments },
        ))
    }

    fn arb_environment_list_result() -> OutputDocumentStrategy {
        serialized_output(
            proptest::collection::vec(arb_environment_with_details(), 0..5).prop_map(
                |environments| crate::model::text::environment::EnvironmentListView {
                    environments,
                },
            ),
        )
    }

    fn arb_environment_sync_deployment_options_result() -> OutputDocumentStrategy {
        serialized_output(any::<bool>().prop_map(|updated| {
            crate::model::text::environment::EnvironmentSyncDeploymentOptionsResult { updated }
        }))
    }

    fn arb_environment_with_details()
    -> BoxedStrategy<golem_common::model::environment::EnvironmentWithDetails> {
        (
            arb_environment_summary(),
            arb_application_summary(),
            arb_account_summary(),
        )
            .prop_map(|(environment, application, account)| {
                golem_common::model::environment::EnvironmentWithDetails {
                    environment,
                    application,
                    account,
                }
            })
            .boxed()
    }

    fn arb_environment_summary()
    -> BoxedStrategy<golem_common::model::environment::EnvironmentSummary> {
        (
            arb_uuid(),
            arb_small_u64(),
            Just("generated-env".to_string()),
            any::<u32>(),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
            proptest::option::of(arb_environment_current_deployment()),
        )
            .prop_map(
                |(
                    id,
                    revision,
                    name,
                    diff_model_version,
                    compatibility_check,
                    version_check,
                    security_overrides,
                    current_deployment,
                )| {
                    golem_common::model::environment::EnvironmentSummary {
                        id: golem_common::model::environment::EnvironmentId(id),
                        revision: golem_common::model::environment::EnvironmentRevision::new(
                            revision,
                        )
                        .expect("generated revision should be valid"),
                        name: golem_common::model::environment::EnvironmentName(name),
                        diff_model_version,
                        compatibility_check,
                        version_check,
                        security_overrides,
                        current_deployment,
                    }
                },
            )
            .boxed()
    }

    fn arb_environment_current_deployment()
    -> BoxedStrategy<golem_common::model::environment::EnvironmentCurrentDeploymentView> {
        (
            arb_small_u64(),
            arb_small_u64(),
            arb_small_string(),
            arb_hash(),
        )
            .prop_map(
                |(revision, deployment_revision, deployment_version, deployment_hash)| {
                    golem_common::model::environment::EnvironmentCurrentDeploymentView {
                        revision: golem_common::model::deployment::CurrentDeploymentRevision::new(
                            revision,
                        )
                        .expect("generated revision should be valid"),
                        deployment_revision:
                            golem_common::model::deployment::DeploymentRevision::new(
                                deployment_revision,
                            )
                            .expect("generated revision should be valid"),
                        deployment_version: golem_common::model::deployment::DeploymentVersion(
                            deployment_version,
                        ),
                        deployment_hash,
                    }
                },
            )
            .boxed()
    }

    fn arb_application_summary()
    -> BoxedStrategy<golem_common::model::application::ApplicationSummary> {
        (arb_uuid(), arb_small_string())
            .prop_map(
                |(id, name)| golem_common::model::application::ApplicationSummary {
                    id: golem_common::model::application::ApplicationId(id),
                    name: golem_common::model::application::ApplicationName(name),
                },
            )
            .boxed()
    }

    fn arb_account_summary() -> BoxedStrategy<golem_common::model::account::AccountSummary> {
        (arb_uuid(), arb_small_string(), arb_small_string())
            .prop_map(
                |(id, name, email)| golem_common::model::account::AccountSummary {
                    id: golem_common::model::account::AccountId(id),
                    name,
                    email: golem_common::model::account::AccountEmail::new(email),
                },
            )
            .boxed()
    }

    fn arb_deployment() -> BoxedStrategy<golem_common::model::deployment::Deployment> {
        (arb_uuid(), arb_small_u64(), arb_small_string(), arb_hash())
            .prop_map(|(environment_id, revision, version, deployment_hash)| {
                golem_common::model::deployment::Deployment {
                    environment_id: golem_common::model::environment::EnvironmentId(environment_id),
                    revision: golem_common::model::deployment::DeploymentRevision::new(revision)
                        .expect("generated revision should be valid"),
                    version: golem_common::model::deployment::DeploymentVersion(version),
                    deployment_hash,
                }
            })
            .boxed()
    }

    fn arb_current_deployment() -> BoxedStrategy<golem_common::model::deployment::CurrentDeployment>
    {
        (
            arb_uuid(),
            arb_small_u64(),
            arb_small_string(),
            arb_hash(),
            arb_small_u64(),
        )
            .prop_map(
                |(environment_id, revision, version, deployment_hash, current_revision)| {
                    golem_common::model::deployment::CurrentDeployment {
                        environment_id: golem_common::model::environment::EnvironmentId(
                            environment_id,
                        ),
                        revision: golem_common::model::deployment::DeploymentRevision::new(
                            revision,
                        )
                        .expect("generated revision should be valid"),
                        version: golem_common::model::deployment::DeploymentVersion(version),
                        deployment_hash,
                        current_revision:
                            golem_common::model::deployment::CurrentDeploymentRevision::new(
                                current_revision,
                            )
                            .expect("generated revision should be valid"),
                        validation_warnings: Vec::new(),
                    }
                },
            )
            .boxed()
    }

    fn arb_plugin_unregister_result() -> OutputDocumentStrategy {
        serialized_output(
            (
                any::<bool>(),
                arb_uuid(),
                arb_small_string(),
                arb_small_string(),
            )
                .prop_map(|(unregistered, plugin_id, name, version)| {
                    crate::model::text::plugin::PluginUnregisterResult {
                        unregistered,
                        plugin_id,
                        name,
                        version,
                    }
                }),
        )
    }

    fn arb_plugin_get_result() -> OutputDocumentStrategy {
        serialized_output(
            arb_plugin_registration()
                .prop_map(crate::model::text::plugin::PluginRegistrationGetView),
        )
    }

    fn arb_plugin_register_result() -> OutputDocumentStrategy {
        serialized_output(
            arb_plugin_registration()
                .prop_map(crate::model::text::plugin::PluginRegistrationRegisterView),
        )
    }

    fn arb_plugin_list_result() -> OutputDocumentStrategy {
        serialized_output(
            proptest::collection::vec(arb_plugin_list_entry(), 0..5)
                .prop_map(|plugins| crate::model::text::plugin::PluginListView { plugins }),
        )
    }

    fn arb_plugin_list_entry() -> BoxedStrategy<crate::model::text::plugin::PluginListEntry> {
        (arb_plugin_registration(), arb_plugin_source())
            .prop_map(
                |(plugin, source)| crate::model::text::plugin::PluginListEntry { plugin, source },
            )
            .boxed()
    }

    fn arb_plugin_source() -> BoxedStrategy<crate::model::text::plugin::PluginSource> {
        prop_oneof![
            Just(crate::model::text::plugin::PluginSource::Own),
            Just(crate::model::text::plugin::PluginSource::Builtin),
            Just(crate::model::text::plugin::PluginSource::Shared),
        ]
        .boxed()
    }

    fn arb_plugin_registration()
    -> BoxedStrategy<golem_common::model::plugin_registration::PluginRegistrationDto> {
        (
            arb_uuid(),
            arb_uuid(),
            arb_small_string(),
            arb_small_string(),
            arb_small_string(),
            Just(golem_common::model::base64::Base64(vec![0])),
            arb_small_string(),
            arb_uuid(),
            arb_small_u64(),
        )
            .prop_map(|(id, account_id, name, version, description, icon, homepage, component_id, component_revision)| {
                golem_common::model::plugin_registration::PluginRegistrationDto {
                    id: golem_common::model::plugin_registration::PluginRegistrationId(id),
                    account_id: golem_common::model::account::AccountId(account_id),
                    name,
                    version,
                    description,
                    icon,
                    homepage,
                    spec: golem_common::model::plugin_registration::PluginSpecDto::OplogProcessor(
                        golem_common::model::plugin_registration::OplogProcessorPluginSpec {
                            component_id: golem_common::model::component::ComponentId(component_id),
                            component_revision: golem_common::model::component::ComponentRevision::new(component_revision)
                                .expect("generated revision should be valid"),
                        },
                    ),
                }
            })
            .boxed()
    }

    fn arb_profile_create_result() -> OutputDocumentStrategy {
        serialized_output((any::<bool>(), arb_small_string(), any::<bool>()).prop_map(
            |(created, profile, set_active)| crate::model::text::profile::ProfileCreateResult {
                created,
                profile: crate::config::ProfileName(profile),
                set_active,
            },
        ))
    }

    fn arb_profile_get_result() -> OutputDocumentStrategy {
        serialized_output(arb_profile_view())
    }

    fn arb_profile_list_result() -> OutputDocumentStrategy {
        serialized_output(
            proptest::collection::vec(arb_profile_view(), 0..5)
                .prop_map(|profiles| crate::model::text::profile::ProfileListView { profiles }),
        )
    }

    fn arb_profile_view() -> BoxedStrategy<crate::model::ProfileView> {
        (
            any::<bool>(),
            arb_small_string(),
            proptest::option::of(arb_url_string()),
            proptest::option::of(arb_url_string()),
            any::<bool>(),
            proptest::option::of(any::<bool>()),
            arb_format_string(),
        )
            .prop_map(
                |(
                    is_active,
                    name,
                    url,
                    worker_url,
                    allow_insecure,
                    authenticated,
                    default_format,
                )| {
                    crate::model::ProfileView {
                        is_active,
                        name: crate::config::ProfileName(name),
                        url: url.map(|url| url.parse().expect("generated URL should parse")),
                        worker_url: worker_url
                            .map(|url| url.parse().expect("generated URL should parse")),
                        allow_insecure,
                        authenticated,
                        config: crate::config::ProfileConfig {
                            default_format: default_format
                                .parse()
                                .expect("generated format should parse"),
                        },
                    }
                },
            )
            .boxed()
    }

    fn arb_profile_switch_result() -> OutputDocumentStrategy {
        serialized_output(
            (any::<bool>(), arb_small_string()).prop_map(|(switched, profile)| {
                crate::model::text::profile::ProfileSwitchResult {
                    switched,
                    profile: crate::config::ProfileName(profile),
                }
            }),
        )
    }

    fn arb_profile_delete_result() -> OutputDocumentStrategy {
        serialized_output(
            (any::<bool>(), arb_small_string()).prop_map(|(deleted, profile)| {
                crate::model::text::profile::ProfileDeleteResult {
                    deleted,
                    profile: crate::config::ProfileName(profile),
                }
            }),
        )
    }

    fn arb_profile_config_set_format_result() -> OutputDocumentStrategy {
        serialized_output((any::<bool>(), arb_small_string(), arb_format()).prop_map(
            |(updated, profile, format)| {
                crate::model::text::profile::ProfileConfigSetFormatResult {
                    updated,
                    profile: crate::config::ProfileName(profile),
                    format,
                }
            },
        ))
    }

    fn arb_resource_create_result() -> OutputDocumentStrategy {
        serialized_output(
            arb_resource_definition()
                .prop_map(crate::model::text::resource_definition::ResourceDefinitionCreateView),
        )
    }

    fn arb_resource_delete_result() -> OutputDocumentStrategy {
        serialized_output(
            arb_resource_definition()
                .prop_map(crate::model::text::resource_definition::ResourceDefinitionDeleteView),
        )
    }

    fn arb_resource_get_result() -> OutputDocumentStrategy {
        serialized_output(
            arb_resource_definition()
                .prop_map(crate::model::text::resource_definition::ResourceDefinitionGetView),
        )
    }

    fn arb_resource_update_result() -> OutputDocumentStrategy {
        serialized_output(
            arb_resource_definition()
                .prop_map(crate::model::text::resource_definition::ResourceDefinitionUpdateView),
        )
    }

    fn arb_resource_list_result() -> OutputDocumentStrategy {
        serialized_output(
            proptest::collection::vec(arb_resource_definition(), 0..5).prop_map(|resources| {
                crate::model::text::resource_definition::ResourceDefinitionListView { resources }
            }),
        )
    }

    fn arb_resource_definition() -> BoxedStrategy<golem_common::model::quota::ResourceDefinition> {
        (
            arb_uuid(),
            arb_small_u64(),
            arb_uuid(),
            arb_small_string(),
            arb_resource_limit(),
            arb_enforcement_action(),
            arb_small_string(),
            arb_small_string(),
        )
            .prop_map(
                |(id, revision, environment_id, name, limit, enforcement_action, unit, units)| {
                    golem_common::model::quota::ResourceDefinition {
                        id: golem_common::model::quota::ResourceDefinitionId(id),
                        revision: golem_common::model::quota::ResourceDefinitionRevision::new(
                            revision,
                        )
                        .expect("generated revision should be valid"),
                        environment_id: golem_common::model::environment::EnvironmentId(
                            environment_id,
                        ),
                        name: golem_common::model::quota::ResourceName(name),
                        limit,
                        enforcement_action,
                        unit,
                        units,
                    }
                },
            )
            .boxed()
    }

    fn arb_resource_limit() -> BoxedStrategy<golem_common::model::quota::ResourceLimit> {
        prop_oneof![
            (arb_small_u64(), arb_time_period(), arb_small_u64()).prop_map(
                |(value, period, max)| {
                    golem_common::model::quota::ResourceLimit::Rate(
                        golem_common::model::quota::ResourceRateLimit { value, period, max },
                    )
                }
            ),
            arb_small_u64().prop_map(|value| {
                golem_common::model::quota::ResourceLimit::Capacity(
                    golem_common::model::quota::ResourceCapacityLimit { value },
                )
            }),
            arb_small_u64().prop_map(|value| {
                golem_common::model::quota::ResourceLimit::Concurrency(
                    golem_common::model::quota::ResourceConcurrencyLimit { value },
                )
            }),
        ]
        .boxed()
    }

    fn arb_enforcement_action() -> BoxedStrategy<golem_common::model::quota::EnforcementAction> {
        prop_oneof![
            Just(golem_common::model::quota::EnforcementAction::Reject),
            Just(golem_common::model::quota::EnforcementAction::Throttle),
            Just(golem_common::model::quota::EnforcementAction::Terminate),
        ]
        .boxed()
    }

    fn arb_time_period() -> BoxedStrategy<golem_common::model::quota::TimePeriod> {
        prop_oneof![
            Just(golem_common::model::quota::TimePeriod::Second),
            Just(golem_common::model::quota::TimePeriod::Minute),
            Just(golem_common::model::quota::TimePeriod::Hour),
            Just(golem_common::model::quota::TimePeriod::Day),
            Just(golem_common::model::quota::TimePeriod::Month),
            Just(golem_common::model::quota::TimePeriod::Year)
        ]
        .boxed()
    }

    fn arb_api_predicate_value()
    -> BoxedStrategy<golem_common::base_model::retry_policy::ApiPredicateValue> {
        prop_oneof![
            arb_small_string().prop_map(|value| {
                golem_common::base_model::retry_policy::ApiPredicateValue::Text(
                    golem_common::base_model::retry_policy::ApiTextValue { value },
                )
            }),
            any::<i64>().prop_map(|value| {
                golem_common::base_model::retry_policy::ApiPredicateValue::Integer(
                    golem_common::base_model::retry_policy::ApiIntegerValue { value },
                )
            }),
            any::<bool>().prop_map(|value| {
                golem_common::base_model::retry_policy::ApiPredicateValue::Boolean(
                    golem_common::base_model::retry_policy::ApiBooleanValue { value },
                )
            }),
        ]
        .boxed()
    }

    fn arb_api_predicate() -> BoxedStrategy<golem_common::base_model::retry_policy::ApiPredicate> {
        arb_api_predicate_with_depth(2)
    }

    fn arb_api_predicate_with_depth(
        depth: u32,
    ) -> BoxedStrategy<golem_common::base_model::retry_policy::ApiPredicate> {
        let leaf = prop_oneof![
            (arb_small_string(), arb_api_predicate_value()).prop_map(|(property, value)| {
                golem_common::base_model::retry_policy::ApiPredicate::PropEq(
                    golem_common::base_model::retry_policy::ApiPropertyComparison {
                        property,
                        value,
                    },
                )
            }),
            (arb_small_string(), arb_api_predicate_value()).prop_map(|(property, value)| {
                golem_common::base_model::retry_policy::ApiPredicate::PropNeq(
                    golem_common::base_model::retry_policy::ApiPropertyComparison {
                        property,
                        value,
                    },
                )
            }),
            (arb_small_string(), arb_api_predicate_value()).prop_map(|(property, value)| {
                golem_common::base_model::retry_policy::ApiPredicate::PropGt(
                    golem_common::base_model::retry_policy::ApiPropertyComparison {
                        property,
                        value,
                    },
                )
            }),
            (arb_small_string(), arb_api_predicate_value()).prop_map(|(property, value)| {
                golem_common::base_model::retry_policy::ApiPredicate::PropGte(
                    golem_common::base_model::retry_policy::ApiPropertyComparison {
                        property,
                        value,
                    },
                )
            }),
            (arb_small_string(), arb_api_predicate_value()).prop_map(|(property, value)| {
                golem_common::base_model::retry_policy::ApiPredicate::PropLt(
                    golem_common::base_model::retry_policy::ApiPropertyComparison {
                        property,
                        value,
                    },
                )
            }),
            (arb_small_string(), arb_api_predicate_value()).prop_map(|(property, value)| {
                golem_common::base_model::retry_policy::ApiPredicate::PropLte(
                    golem_common::base_model::retry_policy::ApiPropertyComparison {
                        property,
                        value,
                    },
                )
            }),
            arb_small_string().prop_map(|property| {
                golem_common::base_model::retry_policy::ApiPredicate::PropExists(
                    golem_common::base_model::retry_policy::ApiPropertyExistence { property },
                )
            }),
            (
                arb_small_string(),
                proptest::collection::vec(arb_api_predicate_value(), 0..4),
            )
                .prop_map(|(property, values)| {
                    golem_common::base_model::retry_policy::ApiPredicate::PropIn(
                        golem_common::base_model::retry_policy::ApiPropertySetCheck {
                            property,
                            values,
                        },
                    )
                }),
            (arb_small_string(), arb_small_string()).prop_map(|(property, pattern)| {
                golem_common::base_model::retry_policy::ApiPredicate::PropMatches(
                    golem_common::base_model::retry_policy::ApiPropertyPattern {
                        property,
                        pattern,
                    },
                )
            }),
            (arb_small_string(), arb_small_string()).prop_map(|(property, prefix)| {
                golem_common::base_model::retry_policy::ApiPredicate::PropStartsWith(
                    golem_common::base_model::retry_policy::ApiPropertyPrefix { property, prefix },
                )
            }),
            (arb_small_string(), arb_small_string()).prop_map(|(property, substring)| {
                golem_common::base_model::retry_policy::ApiPredicate::PropContains(
                    golem_common::base_model::retry_policy::ApiPropertySubstring {
                        property,
                        substring,
                    },
                )
            }),
            Just(golem_common::base_model::retry_policy::ApiPredicate::True(
                golem_common::base_model::retry_policy::ApiPredicateTrue {},
            )),
            Just(golem_common::base_model::retry_policy::ApiPredicate::False(
                golem_common::base_model::retry_policy::ApiPredicateFalse {},
            )),
        ]
        .boxed();

        if depth == 0 {
            return leaf;
        }

        let inner = arb_api_predicate_with_depth(depth - 1);
        prop_oneof![
            leaf,
            (inner.clone(), inner.clone()).prop_map(|(left, right)| {
                golem_common::base_model::retry_policy::ApiPredicate::And(
                    golem_common::base_model::retry_policy::ApiPredicatePair {
                        left: Box::new(left),
                        right: Box::new(right),
                    },
                )
            }),
            (inner.clone(), inner.clone()).prop_map(|(left, right)| {
                golem_common::base_model::retry_policy::ApiPredicate::Or(
                    golem_common::base_model::retry_policy::ApiPredicatePair {
                        left: Box::new(left),
                        right: Box::new(right),
                    },
                )
            }),
            inner.prop_map(|predicate| {
                golem_common::base_model::retry_policy::ApiPredicate::Not(
                    golem_common::base_model::retry_policy::ApiPredicateNot {
                        predicate: Box::new(predicate),
                    },
                )
            }),
        ]
        .boxed()
    }

    fn arb_api_retry_policy()
    -> BoxedStrategy<golem_common::base_model::retry_policy::ApiRetryPolicy> {
        arb_api_retry_policy_with_depth(2)
    }

    fn arb_api_retry_policy_with_depth(
        depth: u32,
    ) -> BoxedStrategy<golem_common::base_model::retry_policy::ApiRetryPolicy> {
        let leaf = prop_oneof![
            arb_small_u64().prop_map(|delay_ms| {
                golem_common::base_model::retry_policy::ApiRetryPolicy::Periodic(
                    golem_common::base_model::retry_policy::ApiPeriodicPolicy { delay_ms },
                )
            }),
            (arb_small_u64(), 0.0f64..10.0).prop_map(|(base_delay_ms, factor)| {
                golem_common::base_model::retry_policy::ApiRetryPolicy::Exponential(
                    golem_common::base_model::retry_policy::ApiExponentialPolicy {
                        base_delay_ms,
                        factor,
                    },
                )
            }),
            (arb_small_u64(), arb_small_u64()).prop_map(|(first_ms, second_ms)| {
                golem_common::base_model::retry_policy::ApiRetryPolicy::Fibonacci(
                    golem_common::base_model::retry_policy::ApiFibonacciPolicy {
                        first_ms,
                        second_ms,
                    },
                )
            }),
            Just(
                golem_common::base_model::retry_policy::ApiRetryPolicy::Immediate(
                    golem_common::base_model::retry_policy::ApiImmediatePolicy {},
                )
            ),
            Just(
                golem_common::base_model::retry_policy::ApiRetryPolicy::Never(
                    golem_common::base_model::retry_policy::ApiNeverPolicy {},
                )
            ),
        ]
        .boxed();

        if depth == 0 {
            return leaf;
        }

        let inner = arb_api_retry_policy_with_depth(depth - 1);
        prop_oneof![
            leaf,
            (any::<u32>(), inner.clone()).prop_map(|(max_retries, inner)| {
                golem_common::base_model::retry_policy::ApiRetryPolicy::CountBox(
                    golem_common::base_model::retry_policy::ApiCountBoxPolicy {
                        max_retries,
                        inner: Box::new(inner),
                    },
                )
            }),
            (arb_small_u64(), inner.clone()).prop_map(|(limit_ms, inner)| {
                golem_common::base_model::retry_policy::ApiRetryPolicy::TimeBox(
                    golem_common::base_model::retry_policy::ApiTimeBoxPolicy {
                        limit_ms,
                        inner: Box::new(inner),
                    },
                )
            }),
            (arb_small_u64(), arb_small_u64(), inner.clone()).prop_map(
                |(min_delay_ms, max_delay_ms, inner)| {
                    golem_common::base_model::retry_policy::ApiRetryPolicy::Clamp(
                        golem_common::base_model::retry_policy::ApiClampPolicy {
                            min_delay_ms,
                            max_delay_ms,
                            inner: Box::new(inner),
                        },
                    )
                },
            ),
            (arb_small_u64(), inner.clone()).prop_map(|(delay_ms, inner)| {
                golem_common::base_model::retry_policy::ApiRetryPolicy::AddDelay(
                    golem_common::base_model::retry_policy::ApiAddDelayPolicy {
                        delay_ms,
                        inner: Box::new(inner),
                    },
                )
            }),
            (0.0f64..1.0, inner.clone()).prop_map(|(factor, inner)| {
                golem_common::base_model::retry_policy::ApiRetryPolicy::Jitter(
                    golem_common::base_model::retry_policy::ApiJitterPolicy {
                        factor,
                        inner: Box::new(inner),
                    },
                )
            }),
            (arb_api_predicate(), inner.clone()).prop_map(|(predicate, inner)| {
                golem_common::base_model::retry_policy::ApiRetryPolicy::FilteredOn(
                    golem_common::base_model::retry_policy::ApiFilteredOnPolicy {
                        predicate,
                        inner: Box::new(inner),
                    },
                )
            }),
            (inner.clone(), inner.clone()).prop_map(|(first, second)| {
                golem_common::base_model::retry_policy::ApiRetryPolicy::AndThen(
                    golem_common::base_model::retry_policy::ApiRetryPolicyPair {
                        first: Box::new(first),
                        second: Box::new(second),
                    },
                )
            },),
            (inner.clone(), inner.clone()).prop_map(|(first, second)| {
                golem_common::base_model::retry_policy::ApiRetryPolicy::Union(
                    golem_common::base_model::retry_policy::ApiRetryPolicyPair {
                        first: Box::new(first),
                        second: Box::new(second),
                    },
                )
            }),
            (inner.clone(), inner.clone()).prop_map(|(first, second)| {
                golem_common::base_model::retry_policy::ApiRetryPolicy::Intersect(
                    golem_common::base_model::retry_policy::ApiRetryPolicyPair {
                        first: Box::new(first),
                        second: Box::new(second),
                    },
                )
            }),
        ]
        .boxed()
    }

    fn arb_retry_policy_create_result() -> OutputDocumentStrategy {
        serialized_output(
            arb_retry_policy().prop_map(crate::model::text::retry_policy::RetryPolicyCreateView),
        )
    }

    fn arb_retry_policy_delete_result() -> OutputDocumentStrategy {
        serialized_output(
            arb_retry_policy().prop_map(crate::model::text::retry_policy::RetryPolicyDeleteView),
        )
    }

    fn arb_retry_policy_get_result() -> OutputDocumentStrategy {
        serialized_output(
            arb_retry_policy().prop_map(crate::model::text::retry_policy::RetryPolicyGetView),
        )
    }

    fn arb_retry_policy_update_result() -> OutputDocumentStrategy {
        serialized_output(
            arb_retry_policy().prop_map(crate::model::text::retry_policy::RetryPolicyUpdateView),
        )
    }

    fn arb_retry_policy_list_result() -> OutputDocumentStrategy {
        serialized_output(
            proptest::collection::vec(arb_retry_policy(), 0..5).prop_map(|retry_policies| {
                crate::model::text::retry_policy::RetryPolicyListView { retry_policies }
            }),
        )
    }

    fn arb_retry_policy() -> BoxedStrategy<golem_common::model::retry_policy::RetryPolicyDto> {
        (
            arb_uuid(),
            arb_uuid(),
            arb_small_string(),
            arb_small_u64(),
            any::<u32>(),
            arb_api_predicate(),
            arb_api_retry_policy(),
        )
            .prop_map(
                |(id, environment_id, name, revision, priority, predicate, policy)| {
                    golem_common::model::retry_policy::RetryPolicyDto {
                        id: golem_common::model::retry_policy::RetryPolicyId(id),
                        environment_id: golem_common::model::environment::EnvironmentId(
                            environment_id,
                        ),
                        name,
                        revision: golem_common::model::retry_policy::RetryPolicyRevision::new(
                            revision,
                        )
                        .expect("generated revision should be valid"),
                        priority,
                        predicate: golem_common::model::UntypedJsonBody(
                            serde_json::to_value(predicate)
                                .expect("generated predicate should serialize"),
                        ),
                        policy: golem_common::model::UntypedJsonBody(
                            serde_json::to_value(policy)
                                .expect("generated retry policy should serialize"),
                        ),
                    }
                },
            )
            .boxed()
    }

    fn arb_secret_create_result() -> OutputDocumentStrategy {
        arb_secret()
            .prop_map(|secret| {
                to_structured_output_value_masked(
                    crate::model::text::secret::SecretCreateView(secret.into()),
                    MaskingConfig::hide_secrets(),
                )
                .expect("generated secret create should serialize")
            })
            .boxed()
    }

    fn arb_secret_delete_result() -> OutputDocumentStrategy {
        arb_secret()
            .prop_map(|secret| {
                to_structured_output_value_masked(
                    crate::model::text::secret::SecretDeleteView(secret.into()),
                    MaskingConfig::hide_secrets(),
                )
                .expect("generated secret delete should serialize")
            })
            .boxed()
    }

    fn arb_secret_get_result() -> OutputDocumentStrategy {
        arb_secret()
            .prop_map(|secret| {
                to_structured_output_value_masked(
                    crate::model::text::secret::SecretGetView(secret.into()),
                    MaskingConfig::hide_secrets(),
                )
                .expect("generated secret get should serialize")
            })
            .boxed()
    }

    fn arb_secret_update_value_result() -> OutputDocumentStrategy {
        arb_secret()
            .prop_map(|secret| {
                to_structured_output_value_masked(
                    crate::model::text::secret::SecretUpdateView(secret.into()),
                    MaskingConfig::hide_secrets(),
                )
                .expect("generated secret update should serialize")
            })
            .boxed()
    }

    fn arb_secret_list_result() -> OutputDocumentStrategy {
        proptest::collection::vec(arb_secret(), 0..5)
            .prop_map(|secrets| {
                to_structured_output_value_masked(
                    crate::model::text::secret::SecretListView {
                        secrets: secrets.into_iter().map(Into::into).collect(),
                        environment_name: "generated-environment".to_string(),
                        show_ids: false,
                    },
                    MaskingConfig::hide_secrets(),
                )
                .expect("generated secret list should serialize")
            })
            .boxed()
    }

    fn arb_secret() -> BoxedStrategy<golem_client::model::AgentSecretDto> {
        (
            arb_uuid(),
            arb_uuid(),
            proptest::collection::vec(arb_small_string(), 1..4),
            arb_small_u64(),
            arb_secret_type_and_value(),
        )
            .prop_map(
                |(id, environment_id, path, revision, (secret_type, secret_value))| {
                    golem_client::model::AgentSecretDto {
                        id: golem_common::model::agent_secret::AgentSecretId(id),
                        environment_id: golem_common::model::environment::EnvironmentId(
                            environment_id,
                        ),
                        path: golem_common::model::agent_secret::CanonicalAgentSecretPath(path),
                        revision: golem_common::model::agent_secret::AgentSecretRevision::new(
                            revision,
                        )
                        .expect("generated revision should be valid"),
                        secret_type,
                        secret_value,
                    }
                },
            )
            .boxed()
    }

    fn arb_secret_type_and_value() -> BoxedStrategy<(
        golem_common::schema::SchemaGraph,
        Option<golem_common::schema::SchemaValue>,
    )> {
        prop_oneof![
            proptest::option::of(
                arb_small_string().prop_map(golem_common::schema::SchemaValue::String)
            )
            .prop_map(|value| {
                (
                    golem_common::schema::SchemaGraph::anonymous(
                        golem_common::schema::SchemaType::string(),
                    ),
                    value,
                )
            }),
            proptest::option::of(any::<bool>().prop_map(golem_common::schema::SchemaValue::Bool))
                .prop_map(|value| {
                    (
                        golem_common::schema::SchemaGraph::anonymous(
                            golem_common::schema::SchemaType::bool(),
                        ),
                        value,
                    )
                }),
            proptest::option::of(arb_small_u64().prop_map(golem_common::schema::SchemaValue::U64))
                .prop_map(|value| {
                    (
                        golem_common::schema::SchemaGraph::anonymous(
                            golem_common::schema::SchemaType::u64(),
                        ),
                        value,
                    )
                }),
            proptest::option::of(proptest::collection::vec(arb_small_u64(), 0..3).prop_map(
                |values| {
                    golem_common::schema::SchemaValue::List {
                        elements: values
                            .into_iter()
                            .map(golem_common::schema::SchemaValue::U64)
                            .collect(),
                    }
                }
            ))
            .prop_map(|value| {
                (
                    golem_common::schema::SchemaGraph::anonymous(
                        golem_common::schema::SchemaType::list(
                            golem_common::schema::SchemaType::u64(),
                        ),
                    ),
                    value,
                )
            }),
            proptest::option::of(proptest::option::of(arb_small_string()).prop_map(|value| {
                golem_common::schema::SchemaValue::Option {
                    inner: value
                        .map(|value| Box::new(golem_common::schema::SchemaValue::String(value))),
                }
            }))
            .prop_map(|value| {
                (
                    golem_common::schema::SchemaGraph::anonymous(
                        golem_common::schema::SchemaType::option(
                            golem_common::schema::SchemaType::string(),
                        ),
                    ),
                    value,
                )
            }),
        ]
        .boxed()
    }

    fn arb_json_value(depth: u32) -> OutputDocumentStrategy {
        let leaf = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(|value| json!(value)),
            arb_small_string().prop_map(Value::String),
        ];

        if depth == 0 {
            return leaf.boxed();
        }

        let inner = arb_json_value(depth - 1);
        prop_oneof![
            leaf,
            proptest::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            proptest::collection::btree_map(arb_small_string(), inner, 0..4)
                .prop_map(|map| Value::Object(map.into_iter().collect())),
        ]
        .boxed()
    }

    fn arb_format_string() -> BoxedStrategy<&'static str> {
        prop_oneof![
            Just("json"),
            Just("pretty-json"),
            Just("yaml"),
            Just("pretty-yaml"),
            Just("text"),
            Just("toon")
        ]
        .boxed()
    }

    fn arb_format() -> BoxedStrategy<crate::model::format::Format> {
        prop_oneof![
            Just(crate::model::format::Format::Json),
            Just(crate::model::format::Format::PrettyJson),
            Just(crate::model::format::Format::Yaml),
            Just(crate::model::format::Format::PrettyYaml),
            Just(crate::model::format::Format::Text),
            Just(crate::model::format::Format::Toon),
        ]
        .boxed()
    }
}
