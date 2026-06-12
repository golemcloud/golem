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

use anyhow::bail;
use serde::Serialize;
use serde_json::{Map, Value};

pub const CLI_OUTPUT_TYPE_FIELD: &str = "$type";

pub trait CliOutput: Serialize {
    const KIND: &'static str;

    fn type_name() -> String {
        Self::KIND.to_string()
    }
}

pub fn to_cli_output_value<Output: CliOutput>(output: &Output) -> anyhow::Result<Value> {
    let value = serde_json::to_value(output)?;
    let type_value = Value::String(Output::type_name());

    match value {
        Value::Object(fields) => Ok(Value::Object(with_cli_output_type::<Output>(
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

fn with_cli_output_type<Output: CliOutput>(
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
    use crate::model::cli_output::CLI_OUTPUT_TYPE_FIELD;
    use proptest::prelude::*;
    use serde_json::{Value, json};
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};
    use syn::{Expr, ImplItem, Item, ItemImpl, Lit, Type};
    use test_r::test;
    use walkdir::WalkDir;

    type OutputDocumentStrategy = BoxedStrategy<Value>;

    struct CliOutputTestEntry {
        rust_type: &'static str,
        output_type: &'static str,
        examples: fn() -> Vec<Value>,
        arbitrary: Option<fn() -> OutputDocumentStrategy>,
    }

    macro_rules! registry_entry {
        ($rust_type:literal, $output_type:literal) => {
            CliOutputTestEntry {
                rust_type: $rust_type,
                output_type: $output_type,
                examples: || vec![minimal_output_document($output_type)],
                arbitrary: None,
            }
        };
        ($rust_type:literal, $output_type:literal, $arbitrary:expr) => {
            CliOutputTestEntry {
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
                arbitrary: Some($arbitrary),
            }
        };
    }

    static CLI_OUTPUT_TEST_REGISTRY: &[CliOutputTestEntry] = &[
        registry_entry!(
            "AccountDeleteResult",
            "account.delete.result",
            arb_account_delete_result
        ),
        registry_entry!("AccountGetView", "account.get.result"),
        registry_entry!("AccountNewView", "account.new.result"),
        registry_entry!(
            "PermissionShareDeleteResult",
            "account.permission-share.delete.result",
            arb_permission_share_delete_result
        ),
        registry_entry!(
            "PermissionShareGetView",
            "account.permission-share.get.result"
        ),
        registry_entry!(
            "PermissionShareListView",
            "account.permission-share.list.result"
        ),
        registry_entry!(
            "PermissionShareNewView",
            "account.permission-share.new.result"
        ),
        registry_entry!(
            "PermissionShareUpdateView",
            "account.permission-share.update.result"
        ),
        registry_entry!("AccountUpdateView", "account.update.result"),
        registry_entry!("AgentTypeView", "agent-type.get.result"),
        registry_entry!("AgentTypeListView", "agent-type.list.result"),
        registry_entry!(
            "AgentCancelInvocationResult",
            "agent.cancel-invocation.result",
            arb_agent_cancel_invocation_result
        ),
        registry_entry!(
            "AgentDeleteResult",
            "agent.delete.result",
            arb_agent_delete_result
        ),
        registry_entry!("WorkerFilesView", "agent.files.result"),
        registry_entry!("WorkerGetView", "agent.get.result"),
        registry_entry!("InvokeResultView", "agent.invoke.result"),
        registry_entry!("AgentsMetadataResponseView", "agent.list.result"),
        registry_entry!("WorkerCreateView", "agent.new.result"),
        registry_entry!("AgentOplogView", "agent.oplog.result"),
        registry_entry!(
            "AgentPluginToggleResult",
            "agent.plugin-toggle.result",
            arb_agent_plugin_toggle_result
        ),
        registry_entry!(
            "AgentRedeployResult",
            "agent.redeploy.result",
            arb_agent_redeploy_result
        ),
        registry_entry!(
            "AgentRevertResult",
            "agent.revert.result",
            arb_agent_revert_result
        ),
        registry_entry!("AgentStreamEvent", "agent.stream.event"),
        registry_entry!("TryUpdateAllWorkersResult", "agent.update.result"),
        registry_entry!(
            "TokenDeleteResult",
            "api-token.delete.result",
            arb_token_delete_result
        ),
        registry_entry!("TokenListView", "api-token.list.result"),
        registry_entry!("TokenNewView", "api-token.new.result"),
        registry_entry!("HttpApiDeploymentGetView", "api.deployment.get.result"),
        registry_entry!("HttpApiDeploymentListView", "api.deployment.list.result"),
        registry_entry!(
            "DomainRegistrationDeleteResult",
            "api.domain.delete.result",
            arb_api_domain_delete_result
        ),
        registry_entry!("HttpApiDomainListView", "api.domain.list.result"),
        registry_entry!("DomainRegistrationNewView", "api.domain.register.result"),
        registry_entry!(
            "HttpSecuritySchemeCreateView",
            "api.security-scheme.create.result"
        ),
        registry_entry!(
            "HttpSecuritySchemeDeleteView",
            "api.security-scheme.delete.result"
        ),
        registry_entry!(
            "HttpSecuritySchemeGetView",
            "api.security-scheme.get.result"
        ),
        registry_entry!(
            "HttpSecuritySchemeListView",
            "api.security-scheme.list.result"
        ),
        registry_entry!(
            "HttpSecuritySchemeUpdateView",
            "api.security-scheme.update.result"
        ),
        registry_entry!("BuildResult", "app.build.result", arb_build_result),
        registry_entry!("CleanResult", "app.clean.result", arb_clean_result),
        registry_entry!("DeployPlanView", "app.deploy-plan.result"),
        registry_entry!("DeployResultView", "app.deploy.result", arb_deploy_result),
        registry_entry!(
            "GenerateBridgeResult",
            "app.generate-bridge.result",
            arb_generate_bridge_result
        ),
        registry_entry!("NewAppResult", "app.new.result", arb_new_app_result),
        registry_entry!("TemplateListView", "app.templates.result"),
        registry_entry!("ComponentGetView", "component.get.result"),
        registry_entry!("ComponentListView", "component.list.result"),
        registry_entry!(
            "ComponentManifestTraceView",
            "component.manifest-trace.result"
        ),
        registry_entry!("DeploymentNewView", "deployment.create.result"),
        registry_entry!("DeploymentDiff", "deployment.diff.result"),
        registry_entry!("DeploymentListView", "deployment.list.result"),
        registry_entry!("EnvironmentListView", "environment.list.result"),
        registry_entry!("EnvironmentSetupPlanView", "environment.setup-plan.result"),
        registry_entry!("PluginRegistrationGetView", "plugin.get.result"),
        registry_entry!("PluginListView", "plugin.list.result"),
        registry_entry!("PluginRegistrationRegisterView", "plugin.register.result"),
        registry_entry!(
            "PluginUnregisterResult",
            "plugin.unregister.result",
            arb_plugin_unregister_result
        ),
        registry_entry!(
            "ProfileConfigSetFormatResult",
            "profile.config.set-format.result",
            arb_profile_config_set_format_result
        ),
        registry_entry!(
            "ProfileDeleteResult",
            "profile.delete.result",
            arb_profile_delete_result
        ),
        registry_entry!("ProfileView", "profile.get.result"),
        registry_entry!("ProfileListView", "profile.list.result"),
        registry_entry!(
            "ProfileCreateResult",
            "profile.new.result",
            arb_profile_create_result
        ),
        registry_entry!(
            "ProfileSwitchResult",
            "profile.switch.result",
            arb_profile_switch_result
        ),
        registry_entry!("ResourceDefinitionCreateView", "resource.create.result"),
        registry_entry!("ResourceDefinitionDeleteView", "resource.delete.result"),
        registry_entry!("ResourceDefinitionGetView", "resource.get.result"),
        registry_entry!("ResourceDefinitionListView", "resource.list.result"),
        registry_entry!("ResourceDefinitionUpdateView", "resource.update.result"),
        registry_entry!("RetryPolicyCreateView", "retry-policy.create.result"),
        registry_entry!("RetryPolicyDeleteView", "retry-policy.delete.result"),
        registry_entry!("RetryPolicyGetView", "retry-policy.get.result"),
        registry_entry!("RetryPolicyListView", "retry-policy.list.result"),
        registry_entry!("RetryPolicyUpdateView", "retry-policy.update.result"),
        registry_entry!("SecretCreateView", "secret.create.result"),
        registry_entry!("SecretDeleteView", "secret.delete.result"),
        registry_entry!("SecretGetView", "secret.get.result"),
        registry_entry!("SecretListView", "secret.list.result"),
        registry_entry!("SecretUpdateView", "secret.update-value.result"),
    ];

    #[derive(Debug, Clone)]
    struct OutputImpl {
        rust_type: String,
        kind: String,
        file: PathBuf,
    }

    impl OutputImpl {
        fn type_name(&self) -> String {
            self.kind.clone()
        }
    }

    #[derive(Default)]
    struct SourceSummary {
        outputs: Vec<OutputImpl>,
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

        assert_eq!(
            definition_types, schema_types,
            "definition keys must match output types"
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

        let unknown_type = json!({ CLI_OUTPUT_TYPE_FIELD: "unknown.result" });
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

        for entry in CLI_OUTPUT_TEST_REGISTRY.iter().filter(|entry| {
            entry.arbitrary.is_some()
                && !definition_allows_additional_properties(
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

        for entry in CLI_OUTPUT_TEST_REGISTRY {
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
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("command-output-schema")
            .join("command-output.schema.json");
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
        serde_json::from_str(&source)
            .unwrap_or_else(|err| panic!("failed to parse {}: {err}", path.display()))
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
        CLI_OUTPUT_TEST_REGISTRY
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

        match trait_name.as_str() {
            "CliOutput" => {
                let mut kind = None;

                for impl_item in &item.items {
                    if let ImplItem::Const(constant) = impl_item {
                        if constant.ident == "KIND" {
                            kind = string_literal(&constant.expr);
                        }
                    }
                }

                summary.outputs.push(OutputImpl {
                    rust_type,
                    kind: kind.unwrap_or_else(|| "<missing-kind>".to_string()),
                    file: file.to_path_buf(),
                });
            }
            _ => {}
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
        let mut parts = kind.split('.').collect::<Vec<_>>();
        let Some(suffix) = parts.pop() else {
            return false;
        };

        matches!(suffix, "result" | "event" | "progress" | "diagnostic")
            && parts.len() >= 2
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

    fn minimal_output_document(output_type: &str) -> Value {
        json!({ CLI_OUTPUT_TYPE_FIELD: output_type })
    }

    fn arb_registered_output_document() -> BoxedStrategy<Value> {
        let strategies = CLI_OUTPUT_TEST_REGISTRY
            .iter()
            .filter_map(|entry| entry.arbitrary.map(|strategy| strategy()))
            .collect::<Vec<_>>();
        proptest::strategy::Union::new(strategies).boxed()
    }

    fn arb_small_string() -> BoxedStrategy<String> {
        "[a-zA-Z0-9._:/() -]{0,40}".prop_map(|value| value).boxed()
    }

    fn arb_bool_field(
        output_type: &'static str,
        field_name: &'static str,
    ) -> OutputDocumentStrategy {
        any::<bool>()
            .prop_map(move |value| json!({ CLI_OUTPUT_TYPE_FIELD: output_type, field_name: value }))
            .boxed()
    }

    fn arb_build_result() -> OutputDocumentStrategy {
        arb_bool_field("app.build.result", "built")
    }

    fn arb_clean_result() -> OutputDocumentStrategy {
        arb_bool_field("app.clean.result", "cleaned")
    }

    fn arb_deploy_result() -> OutputDocumentStrategy {
        arb_bool_field("app.deploy.result", "deployed")
    }

    fn arb_generate_bridge_result() -> OutputDocumentStrategy {
        arb_bool_field("app.generate-bridge.result", "generated")
    }

    fn arb_agent_delete_result() -> OutputDocumentStrategy {
        (any::<bool>(), arb_small_string())
            .prop_map(|(deleted, agent)| {
                json!({ CLI_OUTPUT_TYPE_FIELD: "agent.delete.result", "deleted": deleted, "agent": agent })
            })
            .boxed()
    }

    fn arb_account_delete_result() -> OutputDocumentStrategy {
        (any::<bool>(), arb_small_string())
            .prop_map(|(deleted, account_id)| {
                json!({ CLI_OUTPUT_TYPE_FIELD: "account.delete.result", "deleted": deleted, "accountId": account_id })
            })
            .boxed()
    }

    fn arb_permission_share_delete_result() -> OutputDocumentStrategy {
        (any::<bool>(), arb_small_string())
            .prop_map(|(deleted, permission_share_id)| {
                json!({
                    CLI_OUTPUT_TYPE_FIELD: "account.permission-share.delete.result",
                    "deleted": deleted,
                    "permissionShareId": permission_share_id,
                })
            })
            .boxed()
    }

    fn arb_agent_cancel_invocation_result() -> OutputDocumentStrategy {
        (any::<bool>(), arb_small_string(), arb_small_string())
            .prop_map(|(canceled, agent, idempotency_key)| {
                json!({
                    CLI_OUTPUT_TYPE_FIELD: "agent.cancel-invocation.result",
                    "canceled": canceled,
                    "agent": agent,
                    "idempotencyKey": idempotency_key,
                })
            })
            .boxed()
    }

    fn arb_agent_redeploy_result() -> OutputDocumentStrategy {
        (
            any::<bool>(),
            proptest::collection::vec(arb_small_string(), 0..5),
        )
            .prop_map(|(redeployed, components)| {
                json!({
                    CLI_OUTPUT_TYPE_FIELD: "agent.redeploy.result",
                    "redeployed": redeployed,
                    "components": components,
                })
            })
            .boxed()
    }

    fn arb_agent_revert_result() -> OutputDocumentStrategy {
        (
            any::<bool>(),
            arb_small_string(),
            proptest::option::of(any::<u64>()),
            proptest::option::of(any::<u64>()),
        )
            .prop_map(
                |(reverted, agent, last_oplog_index, number_of_invocations)| {
                    let mut value = json!({
                        CLI_OUTPUT_TYPE_FIELD: "agent.revert.result",
                        "reverted": reverted,
                        "agent": agent,
                    });
                    if let Some(last_oplog_index) = last_oplog_index {
                        value["lastOplogIndex"] = json!(last_oplog_index);
                    }
                    if let Some(number_of_invocations) = number_of_invocations {
                        value["numberOfInvocations"] = json!(number_of_invocations);
                    }
                    value
                },
            )
            .boxed()
    }

    fn arb_agent_plugin_toggle_result() -> OutputDocumentStrategy {
        (
            any::<bool>(),
            arb_small_string(),
            arb_small_string(),
            any::<i32>(),
        )
            .prop_map(|(activated, agent, plugin, priority)| {
                json!({
                    CLI_OUTPUT_TYPE_FIELD: "agent.plugin-toggle.result",
                    "activated": activated,
                    "agent": agent,
                    "plugin": plugin,
                    "priority": priority,
                })
            })
            .boxed()
    }

    fn arb_token_delete_result() -> OutputDocumentStrategy {
        (any::<bool>(), arb_small_string())
            .prop_map(|(deleted, token_id)| {
                json!({ CLI_OUTPUT_TYPE_FIELD: "api-token.delete.result", "deleted": deleted, "tokenId": token_id })
            })
            .boxed()
    }

    fn arb_api_domain_delete_result() -> OutputDocumentStrategy {
        (any::<bool>(), arb_small_string(), arb_small_string())
            .prop_map(|(deleted, domain, id)| {
                json!({
                    CLI_OUTPUT_TYPE_FIELD: "api.domain.delete.result",
                    "deleted": deleted,
                    "domain": domain,
                    "id": id,
                })
            })
            .boxed()
    }

    fn arb_new_app_result() -> OutputDocumentStrategy {
        (any::<bool>(), arb_small_string(), arb_small_string())
            .prop_map(|(created, application_name, application_dir)| {
                json!({
                    CLI_OUTPUT_TYPE_FIELD: "app.new.result",
                    "created": created,
                    "applicationName": application_name,
                    "applicationDir": application_dir,
                })
            })
            .boxed()
    }

    fn arb_plugin_unregister_result() -> OutputDocumentStrategy {
        (
            any::<bool>(),
            arb_small_string(),
            arb_small_string(),
            arb_small_string(),
        )
            .prop_map(|(unregistered, plugin_id, name, version)| {
                json!({
                    CLI_OUTPUT_TYPE_FIELD: "plugin.unregister.result",
                    "unregistered": unregistered,
                    "pluginId": plugin_id,
                    "name": name,
                    "version": version,
                })
            })
            .boxed()
    }

    fn arb_profile_create_result() -> OutputDocumentStrategy {
        (any::<bool>(), arb_small_string(), any::<bool>())
            .prop_map(|(created, profile, set_active)| {
                json!({
                    CLI_OUTPUT_TYPE_FIELD: "profile.new.result",
                    "created": created,
                    "profile": profile,
                    "setActive": set_active,
                })
            })
            .boxed()
    }

    fn arb_profile_switch_result() -> OutputDocumentStrategy {
        (any::<bool>(), arb_small_string())
            .prop_map(|(switched, profile)| {
                json!({ CLI_OUTPUT_TYPE_FIELD: "profile.switch.result", "switched": switched, "profile": profile })
            })
            .boxed()
    }

    fn arb_profile_delete_result() -> OutputDocumentStrategy {
        (any::<bool>(), arb_small_string())
            .prop_map(|(deleted, profile)| {
                json!({ CLI_OUTPUT_TYPE_FIELD: "profile.delete.result", "deleted": deleted, "profile": profile })
            })
            .boxed()
    }

    fn arb_profile_config_set_format_result() -> OutputDocumentStrategy {
        (
            any::<bool>(),
            arb_small_string(),
            prop_oneof![
                Just("json"),
                Just("pretty-json"),
                Just("yaml"),
                Just("pretty-yaml"),
                Just("text"),
                Just("toon")
            ],
        )
            .prop_map(|(updated, profile, format)| {
                json!({
                    CLI_OUTPUT_TYPE_FIELD: "profile.config.set-format.result",
                    "updated": updated,
                    "profile": profile,
                    "format": format,
                })
            })
            .boxed()
    }
}
