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

    fn arb_agent_type_get_result() -> OutputDocumentStrategy {
        (arb_small_string(), arb_small_string(), arb_small_string())
            .prop_map(|(agent_type, constructor, description)| {
                json!({
                    CLI_OUTPUT_TYPE_FIELD: "agent-type.get.result",
                    "agentType": agent_type,
                    "constructor": constructor,
                    "description": description,
                })
            })
            .boxed()
    }

    fn arb_agent_type_list_result() -> OutputDocumentStrategy {
        proptest::collection::vec(arb_deployed_registered_agent_type(), 0..5)
            .prop_map(|agent_types| {
                json!({
                    CLI_OUTPUT_TYPE_FIELD: "agent-type.list.result",
                    "agentTypes": agent_types,
                })
            })
            .boxed()
    }

    fn arb_deployed_registered_agent_type() -> OutputDocumentStrategy {
        (
            arb_json_value(2),
            arb_small_string(),
            any::<u64>(),
            arb_small_string(),
            arb_small_string(),
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
                )| {
                    let mut value = json!({
                        "agentType": agent_type,
                        "implementedBy": {
                            "componentId": component_id,
                            "componentRevision": component_revision,
                            "componentName": component_name,
                            "accountId": account_id,
                            "accountEmail": account_email,
                        }
                    });
                    if let Some(webhook_prefix_authority_and_path) =
                        webhook_prefix_authority_and_path
                    {
                        value["webhookPrefixAuthorityAndPath"] =
                            json!(webhook_prefix_authority_and_path);
                    }
                    value
                },
            )
            .boxed()
    }

    fn arb_agent_files_result() -> OutputDocumentStrategy {
        proptest::collection::vec(arb_file_node(), 0..6)
            .prop_map(|nodes| {
                json!({
                    CLI_OUTPUT_TYPE_FIELD: "agent.files.result",
                    "nodes": nodes,
                })
            })
            .boxed()
    }

    fn arb_file_node() -> OutputDocumentStrategy {
        (
            arb_small_string(),
            arb_small_string(),
            arb_small_string(),
            arb_small_string(),
            any::<u64>(),
        )
            .prop_map(|(name, last_modified, kind, permissions, size)| {
                json!({
                    "name": name,
                    "lastModified": last_modified,
                    "kind": kind,
                    "permissions": permissions,
                    "size": size,
                })
            })
            .boxed()
    }

    fn arb_agent_get_result() -> OutputDocumentStrategy {
        (arb_agent_metadata_view(), any::<bool>())
            .prop_map(|(metadata, precise)| {
                json!({
                    CLI_OUTPUT_TYPE_FIELD: "agent.get.result",
                    "metadata": metadata,
                    "precise": precise,
                })
            })
            .boxed()
    }

    fn arb_agent_invoke_result() -> OutputDocumentStrategy {
        (
            arb_small_string(),
            proptest::option::of(arb_json_value(2)),
            proptest::option::of(proptest::collection::vec(arb_json_value(2), 0..4)),
            proptest::option::of(arb_small_string()),
            proptest::option::of(arb_small_string()),
        )
            .prop_map(
                |(idempotency_key, result_json, results_json, result, result_format)| {
                    let mut value = json!({
                        CLI_OUTPUT_TYPE_FIELD: "agent.invoke.result",
                        "idempotencyKey": idempotency_key,
                    });
                    if let Some(result_json) = result_json {
                        value["resultJson"] = result_json;
                    }
                    if let Some(results_json) = results_json {
                        value["resultsJson"] = json!(results_json);
                    }
                    if let Some(result) = result {
                        value["result"] = json!(result);
                    }
                    if let Some(result_format) = result_format {
                        value["resultFormat"] = json!(result_format);
                    }
                    value
                },
            )
            .boxed()
    }

    fn arb_agent_list_result() -> OutputDocumentStrategy {
        (
            proptest::collection::vec(arb_agent_metadata_view(), 0..5),
            proptest::collection::btree_map(arb_small_string(), arb_small_string(), 0..4),
        )
            .prop_map(|(agents, cursors)| {
                json!({
                    CLI_OUTPUT_TYPE_FIELD: "agent.list.result",
                    "agents": agents,
                    "cursors": cursors,
                })
            })
            .boxed()
    }

    fn arb_agent_new_result() -> OutputDocumentStrategy {
        (arb_small_string(), proptest::option::of(arb_small_string()))
            .prop_map(|(component_name, agent_name)| {
                let mut value = json!({
                    CLI_OUTPUT_TYPE_FIELD: "agent.new.result",
                    "componentName": component_name,
                });
                if let Some(agent_name) = agent_name {
                    value["agentName"] = json!(agent_name);
                }
                value
            })
            .boxed()
    }

    fn arb_agent_oplog_result() -> OutputDocumentStrategy {
        proptest::collection::vec((any::<u64>(), arb_json_value(2)), 0..6)
            .prop_map(|entries| {
                let entries = entries
                    .into_iter()
                    .map(|(index, entry)| json!([index, entry]))
                    .collect::<Vec<_>>();
                json!({
                    CLI_OUTPUT_TYPE_FIELD: "agent.oplog.result",
                    "entries": entries,
                })
            })
            .boxed()
    }

    fn arb_agent_stream_event() -> OutputDocumentStrategy {
        (
            arb_small_string(),
            arb_agent_stream_event_kind(),
            arb_small_string(),
            arb_small_string(),
            arb_small_string(),
            proptest::option::of(arb_small_string()),
            proptest::option::of(arb_small_string()),
            proptest::option::of(any::<u64>()),
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
                )| {
                    let mut value = json!({
                        CLI_OUTPUT_TYPE_FIELD: "agent.stream.event",
                        "timestamp": timestamp,
                        "kind": kind,
                        "level": level,
                        "context": context,
                        "message": message,
                    });
                    if let Some(function_name) = function_name {
                        value["functionName"] = json!(function_name);
                    }
                    if let Some(idempotency_key) = idempotency_key {
                        value["idempotencyKey"] = json!(idempotency_key);
                    }
                    if let Some(number_of_missed_messages) = number_of_missed_messages {
                        value["numberOfMissedMessages"] = json!(number_of_missed_messages);
                    }
                    if let Some(error) = error {
                        value["error"] = json!(error);
                    }
                    value
                },
            )
            .boxed()
    }

    fn arb_agent_stream_event_kind() -> BoxedStrategy<&'static str> {
        prop_oneof![
            Just("log"),
            Just("stdout"),
            Just("stderr"),
            Just("stream-closed"),
            Just("stream-error"),
            Just("invocation-started"),
            Just("invocation-finished"),
            Just("missed-messages"),
        ]
        .boxed()
    }

    fn arb_agent_update_result() -> OutputDocumentStrategy {
        (
            proptest::collection::vec(arb_worker_update_attempt(), 0..5),
            proptest::collection::vec(arb_worker_update_attempt(), 0..5),
        )
            .prop_map(|(triggered, failed)| {
                json!({
                    CLI_OUTPUT_TYPE_FIELD: "agent.update.result",
                    "triggered": triggered,
                    "failed": failed,
                })
            })
            .boxed()
    }

    fn arb_worker_update_attempt() -> OutputDocumentStrategy {
        (
            arb_small_string(),
            any::<u64>(),
            arb_small_string(),
            proptest::option::of(arb_small_string()),
        )
            .prop_map(|(component_name, target_revision, agent_name, error)| {
                let mut value = json!({
                    "componentName": component_name,
                    "targetRevision": target_revision,
                    "agentName": agent_name,
                });
                if let Some(error) = error {
                    value["error"] = json!(error);
                }
                value
            })
            .boxed()
    }

    fn arb_agent_metadata_view() -> OutputDocumentStrategy {
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
                arb_small_string(),
            ),
            (
                any::<u64>(),
                any::<u32>(),
                any::<u64>(),
                proptest::collection::vec(arb_update_record(), 0..4),
                arb_small_string(),
                proptest::option::of(arb_small_string()),
                any::<u64>(),
                any::<u64>(),
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

                let mut value = json!({
                    "componentName": component_name,
                    "agentName": agent_name,
                    "createdBy": created_by,
                    "environmentId": environment_id,
                    "env": env,
                    "defaultEnv": default_env,
                    "config": config,
                    "defaultConfig": default_config,
                    "status": status,
                    "componentRevision": component_revision,
                    "retryCount": retry_count,
                    "pendingInvocationCount": pending_invocation_count,
                    "updates": updates,
                    "createdAt": created_at,
                    "componentSize": component_size,
                    "totalLinearMemorySize": total_linear_memory_size,
                    "exportedResourceInstances": exported_resource_instances,
                });
                if let Some(last_error) = last_error {
                    value["lastError"] = json!(last_error);
                }
                value
            })
            .boxed()
    }

    fn arb_agent_config_entry_dto() -> OutputDocumentStrategy {
        (
            proptest::collection::vec(arb_small_string(), 1..4),
            arb_json_value(2),
        )
            .prop_map(|(path, value)| {
                json!({
                    "path": path,
                    "value": value,
                })
            })
            .boxed()
    }

    fn arb_update_record() -> OutputDocumentStrategy {
        prop_oneof![
            (arb_small_string(), any::<u64>()).prop_map(|(timestamp, target_revision)| {
                json!({
                    "type": "PendingUpdate",
                    "timestamp": timestamp,
                    "targetRevision": target_revision,
                })
            }),
            (arb_small_string(), any::<u64>()).prop_map(|(timestamp, target_revision)| {
                json!({
                    "type": "SuccessfulUpdate",
                    "timestamp": timestamp,
                    "targetRevision": target_revision,
                })
            }),
            (
                arb_small_string(),
                any::<u64>(),
                proptest::option::of(arb_small_string()),
            )
                .prop_map(|(timestamp, target_revision, details)| {
                    let mut value = json!({
                        "type": "FailedUpdate",
                        "timestamp": timestamp,
                        "targetRevision": target_revision,
                    });
                    if let Some(details) = details {
                        value["details"] = json!(details);
                    }
                    value
                }),
        ]
        .boxed()
    }

    fn arb_agent_resource_description() -> OutputDocumentStrategy {
        (arb_small_string(), arb_small_string(), arb_small_string())
            .prop_map(|(created_at, resource_owner, resource_name)| {
                json!({
                    "createdAt": created_at,
                    "resourceOwner": resource_owner,
                    "resourceName": resource_name,
                })
            })
            .boxed()
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

    fn arb_account_get_result() -> OutputDocumentStrategy {
        arb_account_value("account.get.result")
    }

    fn arb_account_new_result() -> OutputDocumentStrategy {
        arb_account_value("account.new.result")
    }

    fn arb_account_update_result() -> OutputDocumentStrategy {
        arb_account_value("account.update.result")
    }

    fn arb_account_value(output_type: &'static str) -> OutputDocumentStrategy {
        (
            arb_small_string(),
            any::<u64>(),
            arb_small_string(),
            arb_small_string(),
            arb_small_string(),
            proptest::collection::vec(arb_account_role(), 0..4),
            arb_small_string(),
        )
            .prop_map(
                move |(id, revision, name, email, plan_id, roles, account_root_card_id)| {
                    json!({
                        CLI_OUTPUT_TYPE_FIELD: output_type,
                        "id": id,
                        "revision": revision,
                        "name": name,
                        "email": email,
                        "planId": plan_id,
                        "roles": roles,
                        "accountRootCardId": account_root_card_id,
                    })
                },
            )
            .boxed()
    }

    fn arb_account_role() -> BoxedStrategy<&'static str> {
        prop_oneof![Just("admin"), Just("marketing-admin")].boxed()
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

    fn arb_permission_share_get_result() -> OutputDocumentStrategy {
        arb_permission_share_value("account.permission-share.get.result")
    }

    fn arb_permission_share_new_result() -> OutputDocumentStrategy {
        arb_permission_share_value("account.permission-share.new.result")
    }

    fn arb_permission_share_update_result() -> OutputDocumentStrategy {
        arb_permission_share_value("account.permission-share.update.result")
    }

    fn arb_permission_share_list_result() -> OutputDocumentStrategy {
        proptest::collection::vec(arb_permission_share_value_without_type(), 0..5)
            .prop_map(|permission_shares| {
                json!({
                    CLI_OUTPUT_TYPE_FIELD: "account.permission-share.list.result",
                    "permissionShares": permission_shares,
                })
            })
            .boxed()
    }

    fn arb_permission_share_value(output_type: &'static str) -> OutputDocumentStrategy {
        arb_permission_share_value_without_type()
            .prop_map(move |mut value| {
                value[CLI_OUTPUT_TYPE_FIELD] = json!(output_type);
                value
            })
            .boxed()
    }

    fn arb_permission_share_value_without_type() -> OutputDocumentStrategy {
        (
            arb_small_string(),
            any::<u64>(),
            arb_small_string(),
            arb_small_string(),
            arb_small_string(),
            proptest::option::of(arb_small_string()),
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
                    let mut value = json!({
                        "id": id,
                        "revision": revision,
                        "ownerAccountId": owner_account_id,
                        "targetAccountId": target_account_id,
                        "name": name,
                        "data": data,
                    });
                    if let Some(current_card_id) = current_card_id {
                        value["currentCardId"] = json!(current_card_id);
                    }
                    value
                },
            )
            .boxed()
    }

    fn arb_permission_share_data() -> OutputDocumentStrategy {
        (
            proptest::collection::vec(arb_small_string(), 0..4),
            proptest::collection::vec(arb_small_string(), 0..4),
            proptest::collection::vec(arb_small_string(), 0..4),
            proptest::collection::vec(arb_small_string(), 0..4),
        )
            .prop_map(
                |(lower_positive, lower_negative, upper_positive, upper_negative)| {
                    json!({
                        "lowerPositive": lower_positive,
                        "lowerNegative": lower_negative,
                        "upperPositive": upper_positive,
                        "upperNegative": upper_negative,
                    })
                },
            )
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

    fn arb_token_list_result() -> OutputDocumentStrategy {
        proptest::collection::vec(arb_token_value(), 0..5)
            .prop_map(|tokens| {
                json!({ CLI_OUTPUT_TYPE_FIELD: "api-token.list.result", "tokens": tokens })
            })
            .boxed()
    }

    fn arb_token_new_result() -> OutputDocumentStrategy {
        arb_token_with_secret_value()
            .prop_map(|mut value| {
                value[CLI_OUTPUT_TYPE_FIELD] = json!("api-token.new.result");
                value
            })
            .boxed()
    }

    fn arb_token_value() -> OutputDocumentStrategy {
        (
            arb_small_string(),
            arb_small_string(),
            arb_small_string(),
            arb_small_string(),
        )
            .prop_map(|(id, account_id, created_at, expires_at)| {
                json!({
                    "id": id,
                    "accountId": account_id,
                    "createdAt": created_at,
                    "expiresAt": expires_at,
                })
            })
            .boxed()
    }

    fn arb_token_with_secret_value() -> OutputDocumentStrategy {
        (
            arb_small_string(),
            arb_small_string(),
            arb_small_string(),
            arb_small_string(),
            arb_small_string(),
        )
            .prop_map(|(id, secret, account_id, created_at, expires_at)| {
                json!({
                    "id": id,
                    "secret": secret,
                    "accountId": account_id,
                    "createdAt": created_at,
                    "expiresAt": expires_at,
                })
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

    fn arb_api_domain_register_result() -> OutputDocumentStrategy {
        arb_domain_registration_value("api.domain.register.result")
    }

    fn arb_api_domain_list_result() -> OutputDocumentStrategy {
        proptest::collection::vec(arb_domain_registration_value_without_type(), 0..5)
            .prop_map(|domains| json!({ CLI_OUTPUT_TYPE_FIELD: "api.domain.list.result", "domains": domains }))
            .boxed()
    }

    fn arb_domain_registration_value(output_type: &'static str) -> OutputDocumentStrategy {
        arb_domain_registration_value_without_type()
            .prop_map(move |mut value| {
                value[CLI_OUTPUT_TYPE_FIELD] = json!(output_type);
                value
            })
            .boxed()
    }

    fn arb_domain_registration_value_without_type() -> OutputDocumentStrategy {
        (arb_small_string(), arb_small_string(), arb_small_string())
            .prop_map(|(id, environment_id, domain)| {
                json!({
                    "id": id,
                    "environmentId": environment_id,
                    "domain": domain,
                })
            })
            .boxed()
    }

    fn arb_api_deployment_get_result() -> OutputDocumentStrategy {
        arb_http_api_deployment_value("api.deployment.get.result")
    }

    fn arb_api_deployment_list_result() -> OutputDocumentStrategy {
        proptest::collection::vec(arb_http_api_deployment_value_without_type(), 0..5)
            .prop_map(|deployments| json!({ CLI_OUTPUT_TYPE_FIELD: "api.deployment.list.result", "deployments": deployments }))
            .boxed()
    }

    fn arb_http_api_deployment_value(output_type: &'static str) -> OutputDocumentStrategy {
        arb_http_api_deployment_value_without_type()
            .prop_map(move |mut value| {
                value[CLI_OUTPUT_TYPE_FIELD] = json!(output_type);
                value
            })
            .boxed()
    }

    fn arb_http_api_deployment_value_without_type() -> OutputDocumentStrategy {
        (
            arb_small_string(),
            any::<u64>(),
            arb_small_string(),
            arb_small_string(),
            arb_small_string(),
            proptest::collection::btree_map(
                arb_small_string(),
                arb_http_api_deployment_agent_options(),
                0..4,
            ),
            arb_small_string(),
            arb_small_string(),
            arb_small_string(),
        )
            .prop_map(
                |(
                    id,
                    revision,
                    environment_id,
                    domain,
                    hash,
                    agents,
                    webhooks_prefix,
                    openapi_endpoint_prefix,
                    created_at,
                )| {
                    json!({
                        "id": id,
                        "revision": revision,
                        "environmentId": environment_id,
                        "domain": domain,
                        "hash": hash,
                        "agents": agents,
                        "webhooksPrefix": webhooks_prefix,
                        "openapiEndpointPrefix": openapi_endpoint_prefix,
                        "createdAt": created_at,
                    })
                },
            )
            .boxed()
    }

    fn arb_http_api_deployment_agent_options() -> BoxedStrategy<Value> {
        proptest::option::of(prop_oneof![
            arb_small_string().prop_map(|header_name| json!({
                "type": "TestSessionHeader",
                "headerName": header_name,
            })),
            arb_small_string().prop_map(|security_scheme| json!({
                "type": "SecurityScheme",
                "securityScheme": security_scheme,
            })),
        ])
        .prop_map(|security| {
            let mut value = json!({});
            if let Some(security) = security {
                value["security"] = security;
            }
            value
        })
        .boxed()
    }

    fn arb_api_security_scheme_create_result() -> OutputDocumentStrategy {
        arb_security_scheme_value("api.security-scheme.create.result")
    }

    fn arb_api_security_scheme_delete_result() -> OutputDocumentStrategy {
        arb_security_scheme_value("api.security-scheme.delete.result")
    }

    fn arb_api_security_scheme_get_result() -> OutputDocumentStrategy {
        arb_security_scheme_value("api.security-scheme.get.result")
    }

    fn arb_api_security_scheme_update_result() -> OutputDocumentStrategy {
        arb_security_scheme_value("api.security-scheme.update.result")
    }

    fn arb_api_security_scheme_list_result() -> OutputDocumentStrategy {
        proptest::collection::vec(arb_security_scheme_value_without_type(), 0..5)
            .prop_map(|security_schemes| {
                json!({
                    CLI_OUTPUT_TYPE_FIELD: "api.security-scheme.list.result",
                    "securitySchemes": security_schemes,
                })
            })
            .boxed()
    }

    fn arb_security_scheme_value(output_type: &'static str) -> OutputDocumentStrategy {
        arb_security_scheme_value_without_type()
            .prop_map(move |mut value| {
                value[CLI_OUTPUT_TYPE_FIELD] = json!(output_type);
                value
            })
            .boxed()
    }

    fn arb_security_scheme_value_without_type() -> OutputDocumentStrategy {
        (
            arb_small_string(),
            any::<u64>(),
            arb_small_string(),
            arb_small_string(),
            arb_security_scheme_provider(),
            arb_small_string(),
            arb_small_string(),
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
                    json!({
                        "id": id,
                        "revision": revision,
                        "name": name,
                        "environmentId": environment_id,
                        "providerType": provider_type,
                        "clientId": client_id,
                        "redirectUrl": redirect_url,
                        "scopes": scopes,
                    })
                },
            )
            .boxed()
    }

    fn arb_security_scheme_provider() -> OutputDocumentStrategy {
        prop_oneof![
            Just(json!({ "type": "Google" })),
            Just(json!({ "type": "Facebook" })),
            Just(json!({ "type": "Microsoft" })),
            Just(json!({ "type": "Gitlab" })),
            (arb_small_string(), arb_small_string()).prop_map(|(name, issuer_url)| {
                json!({
                    "type": "Custom",
                    "name": name,
                    "issuerUrl": issuer_url,
                })
            }),
        ]
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

    fn arb_template_list_result() -> OutputDocumentStrategy {
        proptest::collection::vec(arb_template_description(), 0..5)
            .prop_map(|templates| {
                json!({
                    CLI_OUTPUT_TYPE_FIELD: "app.templates.result",
                    "templates": templates,
                })
            })
            .boxed()
    }

    fn arb_template_description() -> OutputDocumentStrategy {
        (arb_small_string(), arb_guest_language(), arb_small_string())
            .prop_map(|(name, language, description)| {
                json!({
                    "name": name,
                    "language": language,
                    "description": description,
                })
            })
            .boxed()
    }

    fn arb_guest_language() -> BoxedStrategy<&'static str> {
        prop_oneof![
            Just("TypeScript"),
            Just("Rust"),
            Just("Scala"),
            Just("MoonBit")
        ]
        .boxed()
    }

    fn arb_component_get_result() -> OutputDocumentStrategy {
        arb_component_view("component.get.result")
    }

    fn arb_component_list_result() -> OutputDocumentStrategy {
        proptest::collection::vec(arb_component_view_without_type(), 0..5)
            .prop_map(|components| {
                json!({
                    CLI_OUTPUT_TYPE_FIELD: "component.list.result",
                    "components": components,
                })
            })
            .boxed()
    }

    fn arb_component_manifest_trace_result() -> OutputDocumentStrategy {
        (arb_small_string(), arb_json_value(2))
            .prop_map(|(component_name, properties)| {
                json!({
                    CLI_OUTPUT_TYPE_FIELD: "component.manifest-trace.result",
                    "componentName": component_name,
                    "properties": properties,
                })
            })
            .boxed()
    }

    fn arb_component_view(output_type: &'static str) -> OutputDocumentStrategy {
        arb_component_view_without_type()
            .prop_map(move |mut value| {
                value[CLI_OUTPUT_TYPE_FIELD] = json!(output_type);
                value
            })
            .boxed()
    }

    fn arb_component_view_without_type() -> OutputDocumentStrategy {
        (
            arb_small_string(),
            arb_small_string(),
            proptest::option::of(arb_small_string()),
            any::<u64>(),
            any::<u64>(),
            arb_small_string(),
            arb_small_string(),
            proptest::collection::vec(arb_small_string(), 0..5),
            proptest::collection::vec(arb_json_value(1), 0..3),
            proptest::collection::btree_map(
                arb_small_string(),
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
                    json!({
                        "componentName": component_name,
                        "componentId": component_id,
                        "componentVersion": component_version,
                        "componentRevision": component_revision,
                        "componentSize": component_size,
                        "createdAt": created_at,
                        "environmentId": environment_id,
                        "exports": exports,
                        "agentTypes": agent_types,
                        "agentTypeProvisionConfigs": agent_type_provision_configs,
                    })
                },
            )
            .boxed()
    }

    fn arb_agent_type_provision_config() -> OutputDocumentStrategy {
        (
            proptest::collection::btree_map(arb_small_string(), arb_small_string(), 0..4),
            proptest::collection::vec(arb_typed_agent_config_entry(), 0..4),
            proptest::collection::vec(arb_installed_plugin(), 0..3),
            proptest::collection::vec(arb_initial_agent_file(), 0..4),
        )
            .prop_map(|(env, config, plugins, files)| {
                json!({
                    "env": env,
                    "config": config,
                    "plugins": plugins,
                    "files": files,
                })
            })
            .boxed()
    }

    fn arb_typed_agent_config_entry() -> OutputDocumentStrategy {
        (
            proptest::collection::vec(arb_small_string(), 1..4),
            arb_json_value(2),
        )
            .prop_map(|(path, value)| {
                json!({
                    "path": path,
                    "value": value,
                })
            })
            .boxed()
    }

    fn arb_installed_plugin() -> OutputDocumentStrategy {
        (
            arb_small_string(),
            any::<u32>(),
            proptest::collection::btree_map(arb_small_string(), arb_small_string(), 0..4),
            arb_small_string(),
            arb_small_string(),
            arb_small_string(),
            proptest::option::of(arb_small_string()),
            proptest::option::of(any::<u64>()),
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
                    let mut value = json!({
                        "environmentPluginGrantId": environment_plugin_grant_id,
                        "priority": priority,
                        "parameters": parameters,
                        "pluginRegistrationId": plugin_registration_id,
                        "pluginName": plugin_name,
                        "pluginVersion": plugin_version,
                    });
                    if let Some(oplog_processor_component_id) = oplog_processor_component_id {
                        value["oplogProcessorComponentId"] = json!(oplog_processor_component_id);
                    }
                    if let Some(oplog_processor_component_revision) =
                        oplog_processor_component_revision
                    {
                        value["oplogProcessorComponentRevision"] =
                            json!(oplog_processor_component_revision);
                    }
                    value
                },
            )
            .boxed()
    }

    fn arb_initial_agent_file() -> OutputDocumentStrategy {
        (
            arb_small_string(),
            arb_small_string(),
            arb_agent_file_permissions(),
            any::<u64>(),
        )
            .prop_map(|(content_hash, path, permissions, size)| {
                json!({
                    "contentHash": content_hash,
                    "path": path,
                    "permissions": permissions,
                    "size": size,
                })
            })
            .boxed()
    }

    fn arb_agent_file_permissions() -> BoxedStrategy<&'static str> {
        prop_oneof![Just("ReadOnly"), Just("ReadWrite")].boxed()
    }

    fn arb_deploy_plan_result() -> OutputDocumentStrategy {
        (
            arb_deployment_diff_payload(),
            proptest::option::of(arb_environment_setup_plan_value()),
        )
            .prop_map(|(deployment_diff, environment_setup)| {
                json!({
                    CLI_OUTPUT_TYPE_FIELD: "app.deploy-plan.result",
                    "deploymentDiff": deployment_diff,
                    "environmentSetup": environment_setup,
                })
            })
            .boxed()
    }

    fn arb_deployment_diff_result() -> OutputDocumentStrategy {
        arb_deployment_diff_payload()
            .prop_map(|deployment_diff| {
                let mut value = json!({ CLI_OUTPUT_TYPE_FIELD: "deployment.diff.result" });
                if let Some(fields) = deployment_diff.as_object() {
                    for (key, field_value) in fields {
                        value[key] = field_value.clone();
                    }
                }
                value
            })
            .boxed()
    }

    fn arb_deployment_diff_payload() -> OutputDocumentStrategy {
        (
            proptest::option::of(arb_deployment_diff_section_value(2)),
            proptest::option::of(arb_deployment_diff_section_value(2)),
            proptest::option::of(arb_deployment_diff_section_value(2)),
        )
            .prop_map(|(components, http_api_deployments, mcp_deployments)| {
                let mut value = json!({});

                if let Some(components) = components {
                    value["components"] = components;
                }
                if let Some(http_api_deployments) = http_api_deployments {
                    value["httpApiDeployments"] = http_api_deployments;
                }
                if let Some(mcp_deployments) = mcp_deployments {
                    value["mcpDeployments"] = mcp_deployments;
                }

                value
            })
            .boxed()
    }

    fn arb_deployment_diff_section_value(depth: u32) -> OutputDocumentStrategy {
        proptest::collection::btree_map(
            arb_small_string(),
            arb_deployment_diff_map_value(depth),
            0..4,
        )
            .prop_map(|entries| Value::Object(entries.into_iter().collect()))
            .boxed()
    }

    fn arb_deployment_diff_map_value(depth: u32) -> OutputDocumentStrategy {
        let base = prop_oneof![
            Just(json!("create")),
            Just(json!("delete")),
            (arb_small_string(), arb_small_string()).prop_map(|(new_hash, current_hash)| {
                json!({
                    "newHash": new_hash,
                    "currentHash": current_hash,
                })
            }),
        ];

        if depth == 0 {
            base.boxed()
        } else {
            prop_oneof![
                base,
                arb_component_diff_payload(depth - 1),
                arb_http_api_deployment_diff_payload(depth - 1),
                arb_mcp_deployment_diff_payload(depth - 1),
            ]
            .boxed()
        }
    }

    fn arb_component_diff_payload(depth: u32) -> OutputDocumentStrategy {
        (
            any::<bool>(),
            proptest::option::of(arb_deployment_diff_section_value(depth)),
        )
            .prop_map(|(wasm_changed, agent_type_provision_config_changes)| {
                let mut value = json!({
                    "wasmChanged": wasm_changed,
                });
                if let Some(agent_type_provision_config_changes) =
                    agent_type_provision_config_changes
                {
                    value["agentTypeProvisionConfigChanges"] = agent_type_provision_config_changes;
                }
                value
            })
            .boxed()
    }

    fn arb_http_api_deployment_diff_payload(depth: u32) -> OutputDocumentStrategy {
        (
            any::<bool>(),
            any::<bool>(),
            proptest::option::of(arb_deployment_diff_section_value(depth)),
        )
            .prop_map(
                |(webhooks_url_changed, openapi_endpoint_changed, agents_changes)| {
                    let mut value = json!({
                        "webhooksUrlChanged": webhooks_url_changed,
                        "openapiEndpointChanged": openapi_endpoint_changed,
                    });
                    if let Some(agents_changes) = agents_changes {
                        value["agentsChanges"] = agents_changes;
                    }
                    value
                },
            )
            .boxed()
    }

    fn arb_mcp_deployment_diff_payload(depth: u32) -> OutputDocumentStrategy {
        proptest::option::of(arb_deployment_diff_section_value(depth))
            .prop_map(|agents_changes| {
                let mut value = json!({});
                if let Some(agents_changes) = agents_changes {
                    value["agentsChanges"] = agents_changes;
                }
                value
            })
            .boxed()
    }

    fn arb_environment_setup_plan_result() -> OutputDocumentStrategy {
        arb_environment_setup_plan_value()
            .prop_map(|mut value| {
                value[CLI_OUTPUT_TYPE_FIELD] = json!("environment.setup-plan.result");
                value
            })
            .boxed()
    }

    fn arb_environment_setup_plan_value() -> OutputDocumentStrategy {
        (
            arb_environment_setup_display_value(),
            proptest::collection::vec(arb_deployment_agent_secret_default_value(), 0..4),
            proptest::collection::vec(arb_deployment_agent_secret_default_value(), 0..4),
            proptest::collection::vec(arb_deployment_retry_policy_default_value(), 0..4),
            proptest::collection::vec(arb_resource_definition_creation_value(), 0..4),
        )
            .prop_map(
                |(
                    display,
                    agent_secret_defaults,
                    skipped_existing_agent_secret_defaults,
                    retry_policy_defaults,
                    resource_defaults,
                )| {
                    json!({
                        "display": display,
                        "agent_secret_defaults": agent_secret_defaults,
                        "skipped_existing_agent_secret_defaults": skipped_existing_agent_secret_defaults,
                        "retry_policy_defaults": retry_policy_defaults,
                        "resource_defaults": resource_defaults,
                    })
                },
            )
            .boxed()
    }

    fn arb_environment_setup_display_value() -> OutputDocumentStrategy {
        (
            proptest::option::of(arb_environment_setup_detailed_section_value()),
            proptest::option::of(arb_environment_setup_keys_only_section_value()),
        )
            .prop_map(|(to_be_applied, skipped_already_exists)| {
                let mut value = json!({});
                if let Some(to_be_applied) = to_be_applied {
                    value["toBeApplied"] = to_be_applied;
                }
                if let Some(skipped_already_exists) = skipped_already_exists {
                    value["skippedAlreadyExists"] = skipped_already_exists;
                }
                value
            })
            .boxed()
    }

    fn arb_environment_setup_detailed_section_value() -> OutputDocumentStrategy {
        (
            proptest::option::of(proptest::collection::btree_map(
                arb_small_string(),
                arb_environment_setup_secret_value_display_value(),
                0..4,
            )),
            proptest::option::of(proptest::collection::btree_map(
                arb_small_string(),
                arb_environment_setup_retry_policy_display_value(),
                0..4,
            )),
            proptest::option::of(proptest::collection::btree_map(
                arb_small_string(),
                arb_environment_setup_resource_display_value(),
                0..4,
            )),
        )
            .prop_map(|(secret_values, retry_policies, resources)| {
                let mut value = json!({});
                if let Some(secret_values) = secret_values {
                    value["secretValues"] = json!(secret_values);
                }
                if let Some(retry_policies) = retry_policies {
                    value["retryPolicies"] = json!(retry_policies);
                }
                if let Some(resources) = resources {
                    value["resources"] = json!(resources);
                }
                value
            })
            .boxed()
    }

    fn arb_environment_setup_keys_only_section_value() -> OutputDocumentStrategy {
        (
            proptest::option::of(proptest::collection::btree_set(arb_small_string(), 0..4)),
            proptest::option::of(proptest::collection::btree_set(arb_small_string(), 0..4)),
            proptest::option::of(proptest::collection::btree_set(arb_small_string(), 0..4)),
        )
            .prop_map(|(secret_values, retry_policies, resources)| {
                let mut value = json!({});
                if let Some(secret_values) = secret_values {
                    value["secretValues"] = json!(secret_values);
                }
                if let Some(retry_policies) = retry_policies {
                    value["retryPolicies"] = json!(retry_policies);
                }
                if let Some(resources) = resources {
                    value["resources"] = json!(resources);
                }
                value
            })
            .boxed()
    }

    fn arb_environment_setup_secret_value_display_value() -> OutputDocumentStrategy {
        (arb_small_string(), arb_json_value(2))
            .prop_map(|(secret_type, value)| {
                json!({
                    "secretType": secret_type,
                    "value": value,
                })
            })
            .boxed()
    }

    fn arb_environment_setup_retry_policy_display_value() -> OutputDocumentStrategy {
        (any::<u32>(), arb_api_predicate(), arb_api_retry_policy())
            .prop_map(|(priority, predicate, policy)| {
                json!({
                    "priority": priority,
                    "predicate": predicate,
                    "policy": policy,
                })
            })
            .boxed()
    }

    fn arb_environment_setup_resource_display_value() -> OutputDocumentStrategy {
        (
            arb_resource_limit(),
            arb_small_string(),
            arb_small_string(),
            arb_small_string(),
        )
            .prop_map(|(limit, enforcement_action, unit, units)| {
                json!({
                    "limit": limit,
                    "enforcementAction": enforcement_action,
                    "unit": unit,
                    "units": units,
                })
            })
            .boxed()
    }

    fn arb_deployment_agent_secret_default_value() -> OutputDocumentStrategy {
        (
            proptest::collection::vec(arb_small_string(), 1..4),
            arb_json_value(2),
        )
            .prop_map(|(path, secret_value)| {
                json!({
                    "path": path,
                    "secretValue": secret_value,
                })
            })
            .boxed()
    }

    fn arb_deployment_retry_policy_default_value() -> OutputDocumentStrategy {
        (
            arb_small_string(),
            any::<u32>(),
            arb_api_predicate(),
            arb_api_retry_policy(),
        )
            .prop_map(|(name, priority, predicate, policy)| {
                json!({
                    "name": name,
                    "priority": priority,
                    "predicate": predicate,
                    "policy": policy,
                })
            })
            .boxed()
    }

    fn arb_resource_definition_creation_value() -> OutputDocumentStrategy {
        (
            arb_small_string(),
            arb_resource_limit(),
            arb_enforcement_action(),
            arb_small_string(),
            arb_small_string(),
        )
            .prop_map(|(name, limit, enforcement_action, unit, units)| {
                json!({
                    "name": name,
                    "limit": limit,
                    "enforcementAction": enforcement_action,
                    "unit": unit,
                    "units": units,
                })
            })
            .boxed()
    }

    fn arb_deployment_create_result() -> OutputDocumentStrategy {
        (
            arb_small_string(),
            arb_small_string(),
            arb_current_deployment_value(),
        )
            .prop_map(|(application_name, environment_name, deployment)| {
                json!({
                    CLI_OUTPUT_TYPE_FIELD: "deployment.create.result",
                    "applicationName": application_name,
                    "environmentName": environment_name,
                    "deployment": deployment,
                })
            })
            .boxed()
    }

    fn arb_deployment_list_result() -> OutputDocumentStrategy {
        proptest::collection::vec(arb_deployment_value(), 0..5)
            .prop_map(|deployments| {
                json!({
                    CLI_OUTPUT_TYPE_FIELD: "deployment.list.result",
                    "deployments": deployments,
                })
            })
            .boxed()
    }

    fn arb_environment_list_result() -> OutputDocumentStrategy {
        proptest::collection::vec(arb_environment_with_details_value(), 0..5)
            .prop_map(|environments| {
                json!({
                    CLI_OUTPUT_TYPE_FIELD: "environment.list.result",
                    "environments": environments,
                })
            })
            .boxed()
    }

    fn arb_environment_with_details_value() -> OutputDocumentStrategy {
        (
            arb_environment_summary_value(),
            arb_application_summary_value(),
            arb_account_summary_value(),
        )
            .prop_map(|(environment, application, account)| {
                json!({
                    "environment": environment,
                    "application": application,
                    "account": account,
                })
            })
            .boxed()
    }

    fn arb_environment_summary_value() -> OutputDocumentStrategy {
        (
            arb_small_string(),
            any::<u64>(),
            arb_small_string(),
            any::<u32>(),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
            proptest::option::of(arb_environment_current_deployment_value()),
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
                    json!({
                        "id": id,
                        "revision": revision,
                        "name": name,
                        "diffModelVersion": diff_model_version,
                        "compatibilityCheck": compatibility_check,
                        "versionCheck": version_check,
                        "securityOverrides": security_overrides,
                        "currentDeployment": current_deployment,
                    })
                },
            )
            .boxed()
    }

    fn arb_environment_current_deployment_value() -> OutputDocumentStrategy {
        (
            any::<u64>(),
            any::<u64>(),
            arb_small_string(),
            arb_small_string(),
        )
            .prop_map(
                |(revision, deployment_revision, deployment_version, deployment_hash)| {
                    json!({
                        "revision": revision,
                        "deploymentRevision": deployment_revision,
                        "deploymentVersion": deployment_version,
                        "deploymentHash": deployment_hash,
                    })
                },
            )
            .boxed()
    }

    fn arb_application_summary_value() -> OutputDocumentStrategy {
        (arb_small_string(), arb_small_string())
            .prop_map(|(id, name)| {
                json!({
                    "id": id,
                    "name": name,
                })
            })
            .boxed()
    }

    fn arb_account_summary_value() -> OutputDocumentStrategy {
        (arb_small_string(), arb_small_string(), arb_small_string())
            .prop_map(|(id, name, email)| {
                json!({
                    "id": id,
                    "name": name,
                    "email": email,
                })
            })
            .boxed()
    }

    fn arb_deployment_value() -> OutputDocumentStrategy {
        (
            arb_small_string(),
            any::<u64>(),
            arb_small_string(),
            arb_small_string(),
        )
            .prop_map(|(environment_id, revision, version, deployment_hash)| {
                json!({
                    "environmentId": environment_id,
                    "revision": revision,
                    "version": version,
                    "deploymentHash": deployment_hash,
                })
            })
            .boxed()
    }

    fn arb_current_deployment_value() -> OutputDocumentStrategy {
        (
            arb_small_string(),
            any::<u64>(),
            arb_small_string(),
            arb_small_string(),
            any::<u64>(),
            proptest::collection::vec(arb_deploy_validation_warning(), 0..4),
        )
            .prop_map(
                |(
                    environment_id,
                    revision,
                    version,
                    deployment_hash,
                    current_revision,
                    validation_warnings,
                )| {
                    json!({
                        "environmentId": environment_id,
                        "revision": revision,
                        "version": version,
                        "deploymentHash": deployment_hash,
                        "currentRevision": current_revision,
                        "validationWarnings": validation_warnings,
                    })
                },
            )
            .boxed()
    }

    fn arb_deploy_validation_warning() -> OutputDocumentStrategy {
        prop_oneof![
            (
                arb_small_string(),
                arb_small_string(),
                arb_small_string(),
                arb_deploy_validation_http_method(),
                arb_small_string(),
            )
                .prop_map(
                    |(component_id, agent_type, method_name, http_method, path)| {
                        json!({
                            "type": "HttpApiReadOnlyMethodBoundToNonGetVerb",
                            "componentId": component_id,
                            "agentType": agent_type,
                            "methodName": method_name,
                            "httpMethod": http_method,
                            "path": path,
                        })
                    },
                ),
            (
                arb_small_string(),
                arb_small_string(),
                arb_small_string(),
                any::<u64>(),
            )
                .prop_map(|(component_id, agent_type, method_name, ttl_nanos)| {
                    json!({
                        "type": "HttpApiReadOnlyTtlBelowOneSecond",
                        "componentId": component_id,
                        "agentType": agent_type,
                        "methodName": method_name,
                        "ttlNanos": ttl_nanos,
                    })
                }),
        ]
        .boxed()
    }

    fn arb_deploy_validation_http_method() -> OutputDocumentStrategy {
        prop_oneof![
            Just(json!({ "type": "Get" })),
            Just(json!({ "type": "Head" })),
            Just(json!({ "type": "Post" })),
            Just(json!({ "type": "Put" })),
            Just(json!({ "type": "Delete" })),
            Just(json!({ "type": "Connect" })),
            Just(json!({ "type": "Options" })),
            Just(json!({ "type": "Trace" })),
            Just(json!({ "type": "Patch" })),
            arb_small_string().prop_map(|value| json!({ "type": "Custom", "value": value })),
        ]
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

    fn arb_plugin_get_result() -> OutputDocumentStrategy {
        arb_plugin_registration_value("plugin.get.result")
    }

    fn arb_plugin_register_result() -> OutputDocumentStrategy {
        arb_plugin_registration_value("plugin.register.result")
    }

    fn arb_plugin_list_result() -> OutputDocumentStrategy {
        proptest::collection::vec(
            (
                arb_plugin_registration_value_without_type(),
                prop_oneof![Just("Own"), Just("Builtin"), Just("Shared")],
            )
                .prop_map(|(plugin, source)| json!({ "plugin": plugin, "source": source })),
            0..5,
        )
        .prop_map(
            |plugins| json!({ CLI_OUTPUT_TYPE_FIELD: "plugin.list.result", "plugins": plugins }),
        )
        .boxed()
    }

    fn arb_plugin_registration_value(output_type: &'static str) -> OutputDocumentStrategy {
        arb_plugin_registration_value_without_type()
            .prop_map(move |mut value| {
                value[CLI_OUTPUT_TYPE_FIELD] = json!(output_type);
                value
            })
            .boxed()
    }

    fn arb_plugin_registration_value_without_type() -> OutputDocumentStrategy {
        (
            arb_small_string(),
            arb_small_string(),
            arb_small_string(),
            arb_small_string(),
            arb_small_string(),
            arb_small_string(),
            arb_small_string(),
            arb_small_string(),
            any::<u64>(),
        )
            .prop_map(
                |(
                    id,
                    account_id,
                    name,
                    version,
                    description,
                    icon,
                    homepage,
                    component_id,
                    component_revision,
                )| {
                    json!({
                        "id": id,
                        "accountId": account_id,
                        "name": name,
                        "version": version,
                        "description": description,
                        "icon": icon,
                        "homepage": homepage,
                        "spec": {
                            "type": "OplogProcessor",
                            "componentId": component_id,
                            "componentRevision": component_revision,
                        },
                    })
                },
            )
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

    fn arb_profile_get_result() -> OutputDocumentStrategy {
        arb_profile_view("profile.get.result", true)
    }

    fn arb_profile_list_result() -> OutputDocumentStrategy {
        proptest::collection::vec(arb_profile_view_value(), 0..5)
            .prop_map(|profiles| {
                json!({
                    CLI_OUTPUT_TYPE_FIELD: "profile.list.result",
                    "profiles": profiles,
                })
            })
            .boxed()
    }

    fn arb_profile_view(output_type: &'static str, include_type: bool) -> OutputDocumentStrategy {
        arb_profile_view_value()
            .prop_map(move |mut value| {
                if include_type {
                    value[CLI_OUTPUT_TYPE_FIELD] = json!(output_type);
                }
                value
            })
            .boxed()
    }

    fn arb_profile_view_value() -> OutputDocumentStrategy {
        (
            any::<bool>(),
            arb_small_string(),
            proptest::option::of(arb_small_string()),
            proptest::option::of(arb_small_string()),
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
                    let mut value = json!({
                        "isActive": is_active,
                        "name": name,
                        "config": {
                            "defaultFormat": default_format,
                        },
                    });
                    if let Some(url) = url {
                        value["url"] = json!(url);
                    }
                    if let Some(worker_url) = worker_url {
                        value["workerUrl"] = json!(worker_url);
                    }
                    if allow_insecure {
                        value["allowInsecure"] = json!(allow_insecure);
                    }
                    if let Some(authenticated) = authenticated {
                        value["authenticated"] = json!(authenticated);
                    }
                    value
                },
            )
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
        (any::<bool>(), arb_small_string(), arb_format_string())
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

    fn arb_resource_create_result() -> OutputDocumentStrategy {
        arb_resource_definition_value("resource.create.result")
    }

    fn arb_resource_delete_result() -> OutputDocumentStrategy {
        arb_resource_definition_value("resource.delete.result")
    }

    fn arb_resource_get_result() -> OutputDocumentStrategy {
        arb_resource_definition_value("resource.get.result")
    }

    fn arb_resource_update_result() -> OutputDocumentStrategy {
        arb_resource_definition_value("resource.update.result")
    }

    fn arb_resource_list_result() -> OutputDocumentStrategy {
        proptest::collection::vec(arb_resource_definition_value_without_type(), 0..5)
            .prop_map(|resources| json!({ CLI_OUTPUT_TYPE_FIELD: "resource.list.result", "resources": resources }))
            .boxed()
    }

    fn arb_resource_definition_value(output_type: &'static str) -> OutputDocumentStrategy {
        arb_resource_definition_value_without_type()
            .prop_map(move |mut value| {
                value[CLI_OUTPUT_TYPE_FIELD] = json!(output_type);
                value
            })
            .boxed()
    }

    fn arb_resource_definition_value_without_type() -> OutputDocumentStrategy {
        (
            arb_small_string(),
            any::<u64>(),
            arb_small_string(),
            arb_small_string(),
            arb_resource_limit(),
            arb_enforcement_action(),
            arb_small_string(),
            arb_small_string(),
        )
            .prop_map(
                |(id, revision, environment_id, name, limit, enforcement_action, unit, units)| {
                    json!({
                        "id": id,
                        "revision": revision,
                        "environmentId": environment_id,
                        "name": name,
                        "limit": limit,
                        "enforcementAction": enforcement_action,
                        "unit": unit,
                        "units": units,
                    })
                },
            )
            .boxed()
    }

    fn arb_resource_limit() -> OutputDocumentStrategy {
        prop_oneof![
            (any::<u64>(), arb_time_period(), any::<u64>()).prop_map(|(value, period, max)| {
                json!({ "type": "Rate", "value": value, "period": period, "max": max })
            }),
            any::<u64>().prop_map(|value| json!({ "type": "Capacity", "value": value })),
            any::<u64>().prop_map(|value| json!({ "type": "Concurrency", "value": value })),
        ]
        .boxed()
    }

    fn arb_enforcement_action() -> BoxedStrategy<&'static str> {
        prop_oneof![Just("reject"), Just("throttle"), Just("terminate")].boxed()
    }

    fn arb_time_period() -> BoxedStrategy<&'static str> {
        prop_oneof![
            Just("second"),
            Just("minute"),
            Just("hour"),
            Just("day"),
            Just("month"),
            Just("year")
        ]
        .boxed()
    }

    fn arb_api_predicate_value() -> OutputDocumentStrategy {
        prop_oneof![
            arb_small_string().prop_map(|value| json!({ "type": "Text", "value": value })),
            any::<i64>().prop_map(|value| json!({ "type": "Integer", "value": value })),
            any::<bool>().prop_map(|value| json!({ "type": "Boolean", "value": value })),
        ]
        .boxed()
    }

    fn arb_api_predicate() -> OutputDocumentStrategy {
        arb_api_predicate_with_depth(2)
    }

    fn arb_api_predicate_with_depth(depth: u32) -> OutputDocumentStrategy {
        let leaf = prop_oneof![
            (arb_small_string(), arb_api_predicate_value()).prop_map(|(property, value)| {
                json!({ "type": "PropEq", "property": property, "value": value })
            }),
            (arb_small_string(), arb_api_predicate_value()).prop_map(|(property, value)| {
                json!({ "type": "PropNeq", "property": property, "value": value })
            }),
            (arb_small_string(), arb_api_predicate_value()).prop_map(|(property, value)| {
                json!({ "type": "PropGt", "property": property, "value": value })
            }),
            (arb_small_string(), arb_api_predicate_value()).prop_map(|(property, value)| {
                json!({ "type": "PropGte", "property": property, "value": value })
            }),
            (arb_small_string(), arb_api_predicate_value()).prop_map(|(property, value)| {
                json!({ "type": "PropLt", "property": property, "value": value })
            }),
            (arb_small_string(), arb_api_predicate_value()).prop_map(|(property, value)| {
                json!({ "type": "PropLte", "property": property, "value": value })
            }),
            arb_small_string()
                .prop_map(|property| json!({ "type": "PropExists", "property": property })),
            (
                arb_small_string(),
                proptest::collection::vec(arb_api_predicate_value(), 0..4),
            )
                .prop_map(|(property, values)| {
                    json!({ "type": "PropIn", "property": property, "values": values })
                }),
            (arb_small_string(), arb_small_string()).prop_map(|(property, pattern)| {
                json!({ "type": "PropMatches", "property": property, "pattern": pattern })
            }),
            (arb_small_string(), arb_small_string()).prop_map(|(property, prefix)| {
                json!({ "type": "PropStartsWith", "property": property, "prefix": prefix })
            }),
            (arb_small_string(), arb_small_string()).prop_map(|(property, substring)| {
                json!({ "type": "PropContains", "property": property, "substring": substring })
            }),
            Just(json!({ "type": "True" })),
            Just(json!({ "type": "False" })),
        ];

        if depth == 0 {
            return leaf.boxed();
        }

        let inner = arb_api_predicate_with_depth(depth - 1);
        prop_oneof![
            leaf,
            (inner.clone(), inner.clone())
                .prop_map(|(left, right)| json!({ "type": "And", "left": left, "right": right })),
            (inner.clone(), inner.clone())
                .prop_map(|(left, right)| json!({ "type": "Or", "left": left, "right": right })),
            inner.prop_map(|predicate| json!({ "type": "Not", "predicate": predicate })),
        ]
        .boxed()
    }

    fn arb_api_retry_policy() -> OutputDocumentStrategy {
        arb_api_retry_policy_with_depth(2)
    }

    fn arb_api_retry_policy_with_depth(depth: u32) -> OutputDocumentStrategy {
        let leaf = prop_oneof![
            any::<u64>().prop_map(|delay_ms| json!({ "type": "Periodic", "delayMs": delay_ms })),
            (any::<u64>(), any::<f64>())
                .prop_map(|(base_delay_ms, factor)| json!({ "type": "Exponential", "baseDelayMs": base_delay_ms, "factor": factor })),
            (any::<u64>(), any::<u64>())
                .prop_map(|(first_ms, second_ms)| json!({ "type": "Fibonacci", "firstMs": first_ms, "secondMs": second_ms })),
            Just(json!({ "type": "Immediate" })),
            Just(json!({ "type": "Never" })),
        ];

        if depth == 0 {
            return leaf.boxed();
        }

        let inner = arb_api_retry_policy_with_depth(depth - 1);
        prop_oneof![
            leaf,
            (any::<u32>(), inner.clone()).prop_map(|(max_retries, inner)| {
                json!({ "type": "CountBox", "maxRetries": max_retries, "inner": inner })
            }),
            (any::<u64>(), inner.clone()).prop_map(|(limit_ms, inner)| {
                json!({ "type": "TimeBox", "limitMs": limit_ms, "inner": inner })
            }),
            (any::<u64>(), any::<u64>(), inner.clone()).prop_map(
                |(min_delay_ms, max_delay_ms, inner)| {
                    json!({ "type": "Clamp", "minDelayMs": min_delay_ms, "maxDelayMs": max_delay_ms, "inner": inner })
                },
            ),
            (any::<u64>(), inner.clone())
                .prop_map(|(delay_ms, inner)| json!({ "type": "AddDelay", "delayMs": delay_ms, "inner": inner })),
            (any::<f64>(), inner.clone())
                .prop_map(|(factor, inner)| json!({ "type": "Jitter", "factor": factor, "inner": inner })),
            (arb_api_predicate(), inner.clone()).prop_map(|(predicate, inner)| {
                json!({ "type": "FilteredOn", "predicate": predicate, "inner": inner })
            }),
            (inner.clone(), inner.clone()).prop_map(
                |(first, second)| json!({ "type": "AndThen", "first": first, "second": second }),
            ),
            (inner.clone(), inner.clone())
                .prop_map(|(first, second)| json!({ "type": "Union", "first": first, "second": second })),
            (inner.clone(), inner.clone()).prop_map(
                |(first, second)| json!({ "type": "Intersect", "first": first, "second": second }),
            ),
        ]
        .boxed()
    }

    fn arb_retry_policy_create_result() -> OutputDocumentStrategy {
        arb_retry_policy_value("retry-policy.create.result")
    }

    fn arb_retry_policy_delete_result() -> OutputDocumentStrategy {
        arb_retry_policy_value("retry-policy.delete.result")
    }

    fn arb_retry_policy_get_result() -> OutputDocumentStrategy {
        arb_retry_policy_value("retry-policy.get.result")
    }

    fn arb_retry_policy_update_result() -> OutputDocumentStrategy {
        arb_retry_policy_value("retry-policy.update.result")
    }

    fn arb_retry_policy_list_result() -> OutputDocumentStrategy {
        proptest::collection::vec(arb_retry_policy_value_without_type(), 0..5)
            .prop_map(|retry_policies| json!({ CLI_OUTPUT_TYPE_FIELD: "retry-policy.list.result", "retryPolicies": retry_policies }))
            .boxed()
    }

    fn arb_retry_policy_value(output_type: &'static str) -> OutputDocumentStrategy {
        arb_retry_policy_value_without_type()
            .prop_map(move |mut value| {
                value[CLI_OUTPUT_TYPE_FIELD] = json!(output_type);
                value
            })
            .boxed()
    }

    fn arb_retry_policy_value_without_type() -> OutputDocumentStrategy {
        (
            arb_small_string(),
            arb_small_string(),
            arb_small_string(),
            any::<u64>(),
            any::<u32>(),
            arb_api_predicate(),
            arb_api_retry_policy(),
        )
            .prop_map(
                |(id, environment_id, name, revision, priority, predicate, policy)| {
                    json!({
                        "id": id,
                        "environmentId": environment_id,
                        "name": name,
                        "revision": revision,
                        "priority": priority,
                        "predicate": predicate,
                        "policy": policy,
                    })
                },
            )
            .boxed()
    }

    fn arb_secret_create_result() -> OutputDocumentStrategy {
        arb_secret_value("secret.create.result")
    }

    fn arb_secret_delete_result() -> OutputDocumentStrategy {
        arb_secret_value("secret.delete.result")
    }

    fn arb_secret_get_result() -> OutputDocumentStrategy {
        arb_secret_value("secret.get.result")
    }

    fn arb_secret_update_value_result() -> OutputDocumentStrategy {
        arb_secret_value("secret.update-value.result")
    }

    fn arb_secret_list_result() -> OutputDocumentStrategy {
        proptest::collection::vec(arb_secret_value_without_type(), 0..5)
            .prop_map(|secrets| json!({ CLI_OUTPUT_TYPE_FIELD: "secret.list.result", "secrets": secrets }))
            .boxed()
    }

    fn arb_secret_value(output_type: &'static str) -> OutputDocumentStrategy {
        arb_secret_value_without_type()
            .prop_map(move |secret| {
                json!({
                    CLI_OUTPUT_TYPE_FIELD: output_type,
                    "secret": secret,
                })
            })
            .boxed()
    }

    fn arb_secret_value_without_type() -> OutputDocumentStrategy {
        (
            arb_small_string(),
            arb_small_string(),
            proptest::collection::vec(arb_small_string(), 1..4),
            any::<u64>(),
            arb_json_value(2),
            proptest::option::of(arb_json_value(2)),
        )
            .prop_map(
                |(id, environment_id, path, revision, secret_type, secret_value)| {
                    let mut value = json!({
                        "id": id,
                        "environmentId": environment_id,
                        "path": path,
                        "revision": revision,
                        "secretType": secret_type,
                    });
                    if let Some(secret_value) = secret_value {
                        value["secretValue"] = secret_value;
                    }
                    value
                },
            )
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
}
