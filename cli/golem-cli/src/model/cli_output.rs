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
    use crate::model::cli_output::{CLI_OUTPUT_TYPE_FIELD, CliOutput, to_cli_output_value};
    use proptest::prelude::*;
    use quote::ToTokens;
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
                examples: || vec![json!({ CLI_OUTPUT_TYPE_FIELD: $output_type })],
                arbitrary: None,
            }
        };
        ($rust_type:literal, $output_type:literal, $arbitrary:expr) => {
            CliOutputTestEntry {
                rust_type: $rust_type,
                output_type: $output_type,
                examples: || {
                    let mut runner = proptest::test_runner::TestRunner::deterministic();
                    vec![($arbitrary)()
                        .new_tree(&mut runner)
                        .expect("example strategy should produce a value")
                        .current()]
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
        registry_entry!(
            "AccountGetView",
            "account.get.result",
            arb_account_get_result
        ),
        registry_entry!(
            "AccountNewView",
            "account.new.result",
            arb_account_new_result
        ),
        registry_entry!(
            "PermissionShareDeleteResult",
            "account.permission-share.delete.result",
            arb_permission_share_delete_result
        ),
        registry_entry!(
            "PermissionShareGetView",
            "account.permission-share.get.result",
            arb_permission_share_get_result
        ),
        registry_entry!(
            "PermissionShareListView",
            "account.permission-share.list.result",
            arb_permission_share_list_result
        ),
        registry_entry!(
            "PermissionShareNewView",
            "account.permission-share.new.result",
            arb_permission_share_new_result
        ),
        registry_entry!(
            "PermissionShareUpdateView",
            "account.permission-share.update.result",
            arb_permission_share_update_result
        ),
        registry_entry!(
            "AccountUpdateView",
            "account.update.result",
            arb_account_update_result
        ),
        registry_entry!(
            "AgentTypeView",
            "agent-type.get.result",
            arb_agent_type_get_result
        ),
        registry_entry!(
            "AgentTypeListView",
            "agent-type.list.result",
            arb_agent_type_list_result
        ),
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
        registry_entry!(
            "WorkerFilesView",
            "agent.files.result",
            arb_agent_files_result
        ),
        registry_entry!("WorkerGetView", "agent.get.result", arb_agent_get_result),
        registry_entry!(
            "InvokeResultView",
            "agent.invoke.result",
            arb_agent_invoke_result
        ),
        registry_entry!(
            "AgentsMetadataResponseView",
            "agent.list.result",
            arb_agent_list_result
        ),
        registry_entry!("WorkerCreateView", "agent.new.result", arb_agent_new_result),
        registry_entry!(
            "AgentOplogView",
            "agent.oplog.result",
            arb_agent_oplog_result
        ),
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
        registry_entry!(
            "AgentStreamEvent",
            "agent.stream.event",
            arb_agent_stream_event
        ),
        registry_entry!(
            "TryUpdateAllWorkersResult",
            "agent.update.result",
            arb_agent_update_result
        ),
        registry_entry!(
            "TokenDeleteResult",
            "api-token.delete.result",
            arb_token_delete_result
        ),
        registry_entry!(
            "TokenListView",
            "api-token.list.result",
            arb_token_list_result
        ),
        registry_entry!("TokenNewView", "api-token.new.result", arb_token_new_result),
        registry_entry!(
            "HttpApiDeploymentGetView",
            "api.deployment.get.result",
            arb_api_deployment_get_result
        ),
        registry_entry!(
            "HttpApiDeploymentListView",
            "api.deployment.list.result",
            arb_api_deployment_list_result
        ),
        registry_entry!(
            "DomainRegistrationDeleteResult",
            "api.domain.delete.result",
            arb_api_domain_delete_result
        ),
        registry_entry!(
            "HttpApiDomainListView",
            "api.domain.list.result",
            arb_api_domain_list_result
        ),
        registry_entry!(
            "DomainRegistrationNewView",
            "api.domain.register.result",
            arb_api_domain_register_result
        ),
        registry_entry!(
            "HttpSecuritySchemeCreateView",
            "api.security-scheme.create.result",
            arb_api_security_scheme_create_result
        ),
        registry_entry!(
            "HttpSecuritySchemeDeleteView",
            "api.security-scheme.delete.result",
            arb_api_security_scheme_delete_result
        ),
        registry_entry!(
            "HttpSecuritySchemeGetView",
            "api.security-scheme.get.result",
            arb_api_security_scheme_get_result
        ),
        registry_entry!(
            "HttpSecuritySchemeListView",
            "api.security-scheme.list.result",
            arb_api_security_scheme_list_result
        ),
        registry_entry!(
            "HttpSecuritySchemeUpdateView",
            "api.security-scheme.update.result",
            arb_api_security_scheme_update_result
        ),
        registry_entry!("BuildResult", "app.build.result", arb_build_result),
        registry_entry!("CleanResult", "app.clean.result", arb_clean_result),
        registry_entry!(
            "DeployPlanView",
            "app.deploy-plan.result",
            arb_deploy_plan_result
        ),
        registry_entry!("DeployResultView", "app.deploy.result", arb_deploy_result),
        registry_entry!(
            "GenerateBridgeResult",
            "app.generate-bridge.result",
            arb_generate_bridge_result
        ),
        registry_entry!("NewAppResult", "app.new.result", arb_new_app_result),
        registry_entry!(
            "TemplateListView",
            "app.templates.result",
            arb_template_list_result
        ),
        registry_entry!(
            "ComponentGetView",
            "component.get.result",
            arb_component_get_result
        ),
        registry_entry!(
            "ComponentListView",
            "component.list.result",
            arb_component_list_result
        ),
        registry_entry!(
            "ComponentManifestTraceView",
            "component.manifest-trace.result",
            arb_component_manifest_trace_result
        ),
        registry_entry!(
            "DeploymentNewView",
            "deployment.create.result",
            arb_deployment_create_result
        ),
        registry_entry!(
            "DeploymentDiff",
            "deployment.diff.result",
            arb_deployment_diff_result
        ),
        registry_entry!(
            "DeploymentListView",
            "deployment.list.result",
            arb_deployment_list_result
        ),
        registry_entry!(
            "EnvironmentListView",
            "environment.list.result",
            arb_environment_list_result
        ),
        registry_entry!(
            "EnvironmentSetupPlanView",
            "environment.setup-plan.result",
            arb_environment_setup_plan_result
        ),
        registry_entry!(
            "PluginRegistrationGetView",
            "plugin.get.result",
            arb_plugin_get_result
        ),
        registry_entry!(
            "PluginListView",
            "plugin.list.result",
            arb_plugin_list_result
        ),
        registry_entry!(
            "PluginRegistrationRegisterView",
            "plugin.register.result",
            arb_plugin_register_result
        ),
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
        registry_entry!("ProfileView", "profile.get.result", arb_profile_get_result),
        registry_entry!(
            "ProfileListView",
            "profile.list.result",
            arb_profile_list_result
        ),
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
        registry_entry!(
            "ResourceDefinitionCreateView",
            "resource.create.result",
            arb_resource_create_result
        ),
        registry_entry!(
            "ResourceDefinitionDeleteView",
            "resource.delete.result",
            arb_resource_delete_result
        ),
        registry_entry!(
            "ResourceDefinitionGetView",
            "resource.get.result",
            arb_resource_get_result
        ),
        registry_entry!(
            "ResourceDefinitionListView",
            "resource.list.result",
            arb_resource_list_result
        ),
        registry_entry!(
            "ResourceDefinitionUpdateView",
            "resource.update.result",
            arb_resource_update_result
        ),
        registry_entry!(
            "RetryPolicyCreateView",
            "retry-policy.create.result",
            arb_retry_policy_create_result
        ),
        registry_entry!(
            "RetryPolicyDeleteView",
            "retry-policy.delete.result",
            arb_retry_policy_delete_result
        ),
        registry_entry!(
            "RetryPolicyGetView",
            "retry-policy.get.result",
            arb_retry_policy_get_result
        ),
        registry_entry!(
            "RetryPolicyListView",
            "retry-policy.list.result",
            arb_retry_policy_list_result
        ),
        registry_entry!(
            "RetryPolicyUpdateView",
            "retry-policy.update.result",
            arb_retry_policy_update_result
        ),
        registry_entry!(
            "SecretCreateView",
            "secret.create.result",
            arb_secret_create_result
        ),
        registry_entry!(
            "SecretDeleteView",
            "secret.delete.result",
            arb_secret_delete_result
        ),
        registry_entry!("SecretGetView", "secret.get.result", arb_secret_get_result),
        registry_entry!(
            "SecretListView",
            "secret.list.result",
            arb_secret_list_result
        ),
        registry_entry!(
            "SecretUpdateView",
            "secret.update-value.result",
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

            if let Some(tuple_field_type) = &output.tuple_field_type {
                if is_known_non_object_type(tuple_field_type) {
                    errors.push(format!(
                        "{} is a CliOutput tuple wrapper around non-object type `{}`; use a named output struct instead",
                        output.rust_type, tuple_field_type,
                    ));
                }
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
        if let syn::Fields::Unnamed(fields) = &item.fields {
            if fields.unnamed.len() == 1 {
                if let Some(ty) = fields
                    .unnamed
                    .first()
                    .map(|field| field.ty.to_token_stream().to_string())
                {
                    summary
                        .tuple_field_types_by_struct
                        .insert(item.ident.to_string(), ty);
                }
            }
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
                    tuple_field_type: None,
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
        let strategies = CLI_OUTPUT_TEST_REGISTRY
            .iter()
            .filter_map(|entry| entry.arbitrary.map(|strategy| strategy()))
            .collect::<Vec<_>>();
        proptest::strategy::Union::new(strategies).boxed()
    }

    fn serialized_output<T>(strategy: impl Strategy<Value = T> + 'static) -> OutputDocumentStrategy
    where
        T: CliOutput + 'static,
    {
        strategy
            .prop_map(|output| {
                to_cli_output_value(&output).expect("generated DTO should serialize")
            })
            .boxed()
    }

    fn render_generated_deployment_diff() -> Value {
        to_cli_output_value(&empty_deployment_diff())
            .expect("generated deployment diff should serialize")
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
        Just("1970-01-01T00:00:00Z".to_string()).boxed()
    }

    fn arb_timestamp() -> BoxedStrategy<golem_common::model::Timestamp> {
        Just(
            "1970-01-01T00:00:00Z"
                .parse::<golem_common::model::Timestamp>()
                .expect("fixed timestamp should parse"),
        )
        .boxed()
    }

    fn arb_url_string() -> BoxedStrategy<String> {
        Just("https://example.com/callback".to_string()).boxed()
    }

    fn fixed_datetime() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("1970-01-01T00:00:00Z")
            .expect("fixed timestamp should parse")
            .with_timezone(&chrono::Utc)
    }

    fn render_empty_agent_type_list() -> Value {
        to_cli_output_value(&crate::model::text::agent::AgentTypeListView {
            agent_types: Vec::new(),
        })
        .expect("generated agent type list should serialize")
    }

    fn render_empty_agent_oplog() -> Value {
        to_cli_output_value(&crate::model::text::worker::AgentOplogView {
            entries: Vec::new(),
        })
        .expect("generated agent oplog should serialize")
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
        Just(render_empty_agent_type_list()).boxed()
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
        arb_small_string()
            .prop_map(|idempotency_key| {
                to_cli_output_value(&crate::model::invoke_result_view::InvokeResultView {
                    idempotency_key,
                    result_json: None,
                    results_json: None,
                    result: None,
                    result_format: None,
                    is_void_result: true,
                })
                .expect("generated invoke result should serialize")
            })
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
                to_cli_output_value(&output).expect("generated DTO should serialize")
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
        Just(render_empty_agent_oplog()).boxed()
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
                Just("Running".to_string()).boxed(),
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
                    _status,
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
                    status: golem_common::model::AgentStatus::Running,
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
        (arb_uuid(), arb_uuid())
            .prop_map(|(id, account_id)| golem_common::model::auth::Token {
                id: golem_common::model::auth::TokenId(id),
                account_id: golem_common::model::account::AccountId(account_id),
                created_at: fixed_datetime(),
                expires_at: fixed_datetime(),
            })
            .boxed()
    }

    fn arb_token_with_secret() -> BoxedStrategy<golem_common::model::auth::TokenWithSecret> {
        (arb_uuid(), arb_uuid())
            .prop_map(
                |(id, account_id)| golem_common::model::auth::TokenWithSecret {
                    id: golem_common::model::auth::TokenId(id),
                    secret: golem_common::model::auth::TokenSecret::trusted(
                        "generated-token-secret".to_string(),
                    ),
                    account_id: golem_common::model::account::AccountId(account_id),
                    created_at: fixed_datetime(),
                    expires_at: fixed_datetime(),
                },
            )
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
            Just(fixed_datetime()),
        )
            .prop_map(|(id, revision, environment_id, domain, agents, webhooks_prefix, openapi_endpoint_prefix, created_at)| {
                golem_client::model::HttpApiDeployment {
                    id: golem_common::model::http_api_deployment::HttpApiDeploymentId(id),
                    revision: golem_common::model::http_api_deployment::HttpApiDeploymentRevision::new(revision)
                        .expect("generated revision should be valid"),
                    environment_id: golem_common::model::environment::EnvironmentId(environment_id),
                    domain: golem_common::model::domain_registration::Domain(domain),
                    hash: golem_common::model::diff::Hash::empty(),
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
            Just("generated-scheme".to_string()),
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
        serialized_output(arb_small_string().prop_map(|component_name| {
            crate::model::text::component::ComponentManifestTraceView {
                component_name: golem_common::model::component::ComponentName(component_name),
                properties: crate::model::app::ComponentLayerProperties::default(),
            }
        }))
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
                )| {
                    crate::model::component::ComponentView {
                        show_sensitive: true,
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
                        agent_types: Vec::new(),
                        agent_type_provision_configs: BTreeMap::new(),
                    }
                },
            )
            .boxed()
    }

    fn arb_deploy_plan_result() -> OutputDocumentStrategy {
        any::<bool>()
            .prop_map(|include_environment_setup| {
                let deployment_diff = empty_deployment_diff();
                let environment_setup = include_environment_setup
                    .then(crate::model::deploy::EnvironmentSetupPlan::default);
                to_cli_output_value(&crate::model::text::diff::DeployPlanView {
                    deployment_diff: &deployment_diff,
                    environment_setup: environment_setup.as_ref(),
                })
                .expect("generated deploy plan should serialize")
            })
            .boxed()
    }

    fn arb_deployment_diff_result() -> OutputDocumentStrategy {
        Just(render_generated_deployment_diff()).boxed()
    }

    fn arb_environment_setup_plan_result() -> OutputDocumentStrategy {
        let output = crate::model::deploy::EnvironmentSetupPlan::default();
        Just(
            to_cli_output_value(&crate::model::text::diff::EnvironmentSetupPlanView(&output))
                .expect("generated environment setup plan should serialize"),
        )
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
            Just(golem_common::model::diff::Hash::empty()),
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
        (
            arb_uuid(),
            arb_small_u64(),
            arb_small_string(),
            Just(golem_common::model::diff::Hash::empty()),
        )
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
            Just(golem_common::model::diff::Hash::empty()),
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
        serialized_output(arb_secret().prop_map(|secret| {
            crate::model::text::secret::SecretCreateView {
                secret,
                show_sensitive: true,
            }
        }))
    }

    fn arb_secret_delete_result() -> OutputDocumentStrategy {
        serialized_output(arb_secret().prop_map(|secret| {
            crate::model::text::secret::SecretDeleteView {
                secret,
                show_sensitive: true,
            }
        }))
    }

    fn arb_secret_get_result() -> OutputDocumentStrategy {
        serialized_output(arb_secret().prop_map(|secret| {
            crate::model::text::secret::SecretGetView {
                secret,
                show_sensitive: true,
            }
        }))
    }

    fn arb_secret_update_value_result() -> OutputDocumentStrategy {
        serialized_output(arb_secret().prop_map(|secret| {
            crate::model::text::secret::SecretUpdateView {
                secret,
                show_sensitive: true,
            }
        }))
    }

    fn arb_secret_list_result() -> OutputDocumentStrategy {
        serialized_output(
            proptest::collection::vec(arb_secret(), 0..5).prop_map(|secrets| {
                crate::model::text::secret::SecretListView {
                    secrets,
                    show_sensitive: true,
                    environment_name: "generated".to_string(),
                    show_ids: true,
                }
            }),
        )
    }

    fn arb_secret() -> BoxedStrategy<golem_client::model::AgentSecretDto> {
        (
            arb_uuid(),
            arb_uuid(),
            proptest::collection::vec(arb_small_string(), 1..4),
            arb_small_u64(),
            proptest::option::of(arb_small_string().prop_map(Value::String)),
        )
            .prop_map(|(id, environment_id, path, revision, secret_value)| {
                golem_client::model::AgentSecretDto {
                    id: golem_common::model::agent_secret::AgentSecretId(id),
                    environment_id: golem_common::model::environment::EnvironmentId(environment_id),
                    path: golem_common::model::agent_secret::CanonicalAgentSecretPath(path),
                    revision: golem_common::model::agent_secret::AgentSecretRevision::new(revision)
                        .expect("generated revision should be valid"),
                    secret_type: golem_wasm::analysis::analysed_type::str(),
                    secret_value,
                }
            })
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
