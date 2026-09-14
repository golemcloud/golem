use super::*;
use crate::model::app_raw::{Application, JSON_SCHEMA_VALIDATOR};
use crate::model::cascade::property::map::MapMergeMode;
use crate::model::cascade::property::vec::VecMergeMode;
use crate::model::format::Format;
use golem_common::base_model::retry_policy::{
    ApiAddDelayPolicy, ApiBooleanValue, ApiClampPolicy, ApiCountBoxPolicy, ApiExponentialPolicy,
    ApiFibonacciPolicy, ApiFilteredOnPolicy, ApiImmediatePolicy, ApiIntegerValue, ApiJitterPolicy,
    ApiNeverPolicy, ApiPeriodicPolicy, ApiPredicate, ApiPredicateFalse, ApiPredicateNot,
    ApiPredicatePair, ApiPredicateTrue, ApiPredicateValue, ApiPropertyComparison,
    ApiPropertyExistence, ApiPropertyPattern, ApiPropertyPrefix, ApiPropertySetCheck,
    ApiPropertySubstring, ApiRetryPolicy, ApiRetryPolicyPair, ApiTextValue, ApiTimeBoxPolicy,
};
use golem_common::model::agent::AgentTypeName;
use golem_common::model::component::{AgentFilePermissions, CanonicalFilePath};
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentName;
use golem_common::model::quota::{
    EnforcementAction, ResourceCapacityLimit, ResourceConcurrencyLimit, ResourceLimit,
    ResourceName, ResourceRateLimit, TimePeriod,
};
use golem_common::model::security_scheme::SecuritySchemeName;
use indexmap::IndexMap;
use proptest::prelude::*;
use proptest::string::string_regex;
use serde_json::Value;
use std::collections::HashMap;
#[allow(unused_imports)]
use test_r::test;
use url::Url;

fn arb_opt<T: Clone + std::fmt::Debug + 'static>(
    strategy: BoxedStrategy<T>,
) -> BoxedStrategy<Option<T>> {
    prop_oneof![3 => strategy.prop_map(Some), 2 => Just(None)].boxed()
}

fn arb_ident() -> BoxedStrategy<String> {
    string_regex("[a-z][a-z0-9_-]{0,12}").unwrap().boxed()
}

fn arb_dns_label() -> BoxedStrategy<String> {
    string_regex("[a-z][a-z0-9-]{0,12}").unwrap().boxed()
}

fn arb_tool_name() -> BoxedStrategy<String> {
    (
        string_regex("[a-z][a-z0-9]{0,6}").unwrap(),
        prop::collection::vec(string_regex("[a-z0-9]{1,6}").unwrap(), 0..=2),
    )
        .prop_map(|(head, tail)| {
            std::iter::once(head)
                .chain(tail)
                .collect::<Vec<_>>()
                .join("-")
        })
        .boxed()
}

fn arb_semver() -> BoxedStrategy<String> {
    (0u8..=9, 0u8..=9, 0u8..=9)
        .prop_map(|(a, b, c)| format!("{a}.{b}.{c}"))
        .boxed()
}

fn arb_json_value() -> BoxedStrategy<Value> {
    let leaf = prop_oneof![
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::from),
        arb_ident().prop_map(Value::String),
    ];

    leaf.prop_recursive(3, 64, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..=3).prop_map(Value::Array),
            prop::collection::btree_map(arb_ident(), inner, 0..=3)
                .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
    .boxed()
}

fn arb_token_list_model() -> BoxedStrategy<LenientTokenList> {
    prop_oneof![
        Just(LenientTokenList::None),
        arb_ident().prop_map(LenientTokenList::String),
        prop::collection::vec(arb_ident(), 1..=3).prop_map(LenientTokenList::List),
    ]
    .boxed()
}

fn arb_map_merge_mode_model() -> BoxedStrategy<MapMergeMode> {
    prop_oneof![
        Just(MapMergeMode::Upsert),
        Just(MapMergeMode::Replace),
        Just(MapMergeMode::Remove),
    ]
    .boxed()
}

fn arb_vec_merge_mode_model() -> BoxedStrategy<VecMergeMode> {
    prop_oneof![
        Just(VecMergeMode::Append),
        Just(VecMergeMode::Prepend),
        Just(VecMergeMode::Replace),
    ]
    .boxed()
}

fn arb_string_index_map_model() -> BoxedStrategy<IndexMap<String, String>> {
    prop::collection::vec((arb_ident(), arb_ident()), 0..=3)
        .prop_map(IndexMap::from_iter)
        .boxed()
}

fn arb_secret_key_scope_model() -> BoxedStrategy<ManifestSecretKeyScope> {
    prop_oneof![
        Just(ManifestSecretKeyScope::All("*".to_string())),
        prop::collection::vec(arb_ident(), 0..=3).prop_map(ManifestSecretKeyScope::Keys),
    ]
    .boxed()
}

fn arb_config_key_scope_model() -> BoxedStrategy<ManifestConfigKeyScope> {
    prop_oneof![
        Just(ManifestConfigKeyScope::All("*".to_string())),
        prop::collection::vec(arb_ident(), 0..=3).prop_map(ManifestConfigKeyScope::Keys),
    ]
    .boxed()
}

fn arb_tool_binding_model() -> BoxedStrategy<ToolBinding> {
    (
        arb_opt(arb_semver()),
        arb_opt(arb_map_merge_mode_model()),
        arb_opt(
            prop::collection::vec((arb_ident(), arb_json_value()), 0..=3)
                .prop_map(IndexMap::from_iter)
                .boxed(),
        ),
        arb_opt(
            arb_ident()
                .prop_map(|name| format!("{name}@example.com"))
                .boxed(),
        ),
        arb_opt(Just(SecretKeyMergeMode::Intersect).boxed()),
        arb_opt(arb_config_key_scope_model()),
        arb_opt(Just(SecretKeyMergeMode::Intersect).boxed()),
        arb_opt(arb_secret_key_scope_model()),
        arb_opt(Just(SecretKeyMergeMode::Intersect).boxed()),
        arb_opt(arb_secret_key_scope_model()),
    )
        .prop_map(
            |(
                version,
                parameters_merge_mode,
                parameters,
                account,
                config_keys_readable_merge_mode,
                config_keys_readable,
                secret_keys_readable_merge_mode,
                secret_keys_readable,
                secret_keys_revealable_merge_mode,
                secret_keys_revealable,
            )| ToolBinding {
                version,
                parameters_merge_mode,
                parameters,
                account,
                config_keys_readable_merge_mode,
                config_keys_readable,
                secret_keys_readable_merge_mode,
                secret_keys_readable,
                secret_keys_revealable_merge_mode,
                secret_keys_revealable,
            },
        )
        .boxed()
}

fn arb_tool_bindings_model() -> BoxedStrategy<IndexMap<String, ToolBinding>> {
    prop::collection::vec((arb_tool_name(), arb_tool_binding_model()), 0..=3)
        .prop_map(IndexMap::from_iter)
        .boxed()
}

fn arb_tool_preset_model() -> BoxedStrategy<ToolPreset> {
    (
        any::<bool>(),
        arb_opt(arb_json_value()),
        arb_opt(arb_map_merge_mode_model()),
        arb_opt(arb_string_index_map_model()),
        arb_opt(arb_vec_merge_mode_model()),
        arb_opt(prop::collection::vec(arb_plugin_installation_model(), 0..=2).boxed()),
        arb_opt(arb_vec_merge_mode_model()),
        arb_opt(prop::collection::vec(arb_initial_component_file_model(), 0..=2).boxed()),
    )
        .prop_map(
            |(
                is_default,
                config,
                env_merge_mode,
                env,
                plugins_merge_mode,
                plugins,
                files_merge_mode,
                files,
            )| ToolPreset {
                default: is_default.then_some(Marker),
                config,
                env_merge_mode,
                env,
                plugins_merge_mode,
                plugins,
                files_merge_mode,
                files,
            },
        )
        .boxed()
}

fn arb_tool_declaration_model() -> BoxedStrategy<ToolDeclaration> {
    (
        arb_token_list_model(),
        arb_opt(arb_json_value()),
        arb_opt(arb_map_merge_mode_model()),
        arb_opt(arb_string_index_map_model()),
        arb_opt(arb_vec_merge_mode_model()),
        arb_opt(prop::collection::vec(arb_plugin_installation_model(), 0..=2).boxed()),
        arb_opt(arb_vec_merge_mode_model()),
        arb_opt(prop::collection::vec(arb_initial_component_file_model(), 0..=2).boxed()),
        prop::collection::vec((arb_ident(), arb_tool_preset_model()), 0..=2)
            .prop_map(IndexMap::from_iter),
    )
        .prop_map(
            |(
                templates,
                config,
                env_merge_mode,
                env,
                plugins_merge_mode,
                plugins,
                files_merge_mode,
                files,
                presets,
            )| ToolDeclaration {
                component: None,
                release: None,
                templates,
                config,
                env_merge_mode,
                env,
                plugins_merge_mode,
                plugins,
                files_merge_mode,
                files,
                presets,
            },
        )
        .boxed()
}

fn arb_tool_declarations_model() -> BoxedStrategy<ToolDeclarations> {
    prop::collection::vec((arb_tool_name(), arb_tool_declaration_model()), 0..=3)
        .prop_map(|entries| {
            ToolDeclarations(IndexMap::from_iter(entries.into_iter().map(
                |(name, declaration)| (name, serde_json::to_value(declaration).unwrap()),
            )))
        })
        .boxed()
}

fn arb_url_model() -> BoxedStrategy<Url> {
    arb_dns_label()
        .prop_filter_map("valid url", |host| {
            Url::parse(&format!("https://{host}.example.com")).ok()
        })
        .boxed()
}

fn arb_external_command_model() -> BoxedStrategy<ExternalCommand> {
    (
        arb_ident(),
        arb_opt(arb_ident()),
        arb_string_index_map_model(),
        prop::collection::vec(arb_ident(), 0..=2),
        prop::collection::vec(arb_ident(), 0..=2),
        prop::collection::vec(arb_ident(), 0..=2),
        prop::collection::vec(arb_ident(), 0..=2),
    )
        .prop_map(
            |(command, dir, env, rmdirs, mkdirs, sources, targets)| ExternalCommand {
                command,
                dir,
                env,
                rmdirs,
                mkdirs,
                sources,
                targets,
            },
        )
        .boxed()
}

fn arb_build_commands_model() -> BoxedStrategy<Vec<BuildCommand>> {
    prop::collection::vec(
        arb_external_command_model().prop_map(BuildCommand::External),
        0..=3,
    )
    .boxed()
}

fn arb_plugin_installation_model() -> BoxedStrategy<PluginInstallation> {
    (
        arb_opt(arb_ident()),
        arb_ident(),
        string_regex("[0-9]+\\.[0-9]+\\.[0-9]+(-[a-z0-9.]+)?").unwrap(),
        arb_string_index_map_model(),
    )
        .prop_map(|(account, name, version, parameters)| PluginInstallation {
            account,
            name,
            version,
            parameters: parameters.into_iter().collect::<HashMap<_, _>>(),
        })
        .boxed()
}

fn arb_initial_component_file_model() -> BoxedStrategy<InitialComponentFile> {
    (arb_ident(), arb_ident(), any::<bool>())
        .prop_map(
            |(source_path, target_name, writable)| InitialComponentFile {
                source_path,
                target_path: CanonicalFilePath::from_abs_str(&format!("/{target_name}")).unwrap(),
                permissions: Some(if writable {
                    AgentFilePermissions::ReadWrite
                } else {
                    AgentFilePermissions::ReadOnly
                }),
            },
        )
        .boxed()
}

fn arb_component_preset_model() -> BoxedStrategy<ComponentPreset> {
    (
        any::<bool>(),
        arb_opt(arb_ident()),
        arb_opt(arb_ident()),
        (
            arb_opt(arb_vec_merge_mode_model()),
            arb_build_commands_model(),
            prop::collection::vec(
                (
                    arb_ident(),
                    prop::collection::vec(arb_external_command_model(), 0..=2),
                ),
                0..=2,
            )
            .prop_map(IndexMap::from_iter),
            prop::collection::vec(arb_ident(), 0..=3),
        ),
        (
            arb_opt(arb_json_value()),
            arb_opt(arb_map_merge_mode_model()),
            arb_opt(arb_string_index_map_model()),
        ),
        (
            arb_opt(arb_vec_merge_mode_model()),
            arb_opt(prop::collection::vec(arb_plugin_installation_model(), 0..=2).boxed()),
            arb_opt(arb_vec_merge_mode_model()),
            arb_opt(prop::collection::vec(arb_initial_component_file_model(), 0..=2).boxed()),
        ),
    )
        .prop_map(
            |(
                is_default,
                component_wasm,
                output_wasm,
                (build_merge_mode, build, custom_commands, clean),
                (config, env_merge_mode, env),
                (plugins_merge_mode, plugins, files_merge_mode, files),
            )| ComponentPreset {
                default: is_default.then_some(Marker),
                component_wasm,
                output_wasm,
                dependencies: ComponentDependencies::default(),
                build_merge_mode,
                build,
                custom_commands,
                clean,
                config,
                initial_card: None,
                env_merge_mode,
                env,
                plugins_merge_mode,
                plugins,
                files_merge_mode,
                files,
            },
        )
        .boxed()
}

fn arb_component_template_model() -> BoxedStrategy<ComponentTemplate> {
    (
        arb_token_list_model(),
        arb_opt(arb_ident()),
        arb_opt(arb_ident()),
        (
            arb_opt(arb_vec_merge_mode_model()),
            arb_build_commands_model(),
            prop::collection::vec(
                (
                    arb_ident(),
                    prop::collection::vec(arb_external_command_model(), 0..=2),
                ),
                0..=2,
            )
            .prop_map(IndexMap::from_iter),
            prop::collection::vec(arb_ident(), 0..=3),
        ),
        (
            arb_opt(arb_json_value()),
            arb_opt(arb_map_merge_mode_model()),
            arb_opt(arb_string_index_map_model()),
        ),
        (
            arb_opt(arb_vec_merge_mode_model()),
            arb_opt(prop::collection::vec(arb_plugin_installation_model(), 0..=2).boxed()),
            arb_opt(arb_vec_merge_mode_model()),
            arb_opt(prop::collection::vec(arb_initial_component_file_model(), 0..=2).boxed()),
        ),
        prop::collection::vec((arb_ident(), arb_component_preset_model()), 0..=2)
            .prop_map(IndexMap::from_iter),
    )
        .prop_map(
            |(
                templates,
                component_wasm,
                output_wasm,
                (build_merge_mode, build, custom_commands, clean),
                (config, env_merge_mode, env),
                (plugins_merge_mode, plugins, files_merge_mode, files),
                presets,
            )| ComponentTemplate {
                templates,
                component_wasm,
                output_wasm,
                dependencies: ComponentDependencies::default(),
                build_merge_mode,
                build,
                custom_commands,
                clean,
                config,
                initial_card: None,
                env_merge_mode,
                env,
                plugins_merge_mode,
                plugins,
                files_merge_mode,
                files,
                presets,
            },
        )
        .boxed()
}

fn arb_component_model() -> BoxedStrategy<Component> {
    (
        arb_token_list_model(),
        arb_opt(arb_ident()),
        arb_opt(arb_ident()),
        arb_opt(arb_ident()),
        (
            arb_opt(arb_vec_merge_mode_model()),
            arb_build_commands_model(),
            prop::collection::vec(
                (
                    arb_ident(),
                    prop::collection::vec(arb_external_command_model(), 0..=2),
                ),
                0..=2,
            )
            .prop_map(IndexMap::from_iter),
            prop::collection::vec(arb_ident(), 0..=3),
        ),
        (
            arb_opt(arb_json_value()),
            arb_opt(arb_map_merge_mode_model()),
            arb_opt(arb_string_index_map_model()),
        ),
        (
            arb_opt(arb_vec_merge_mode_model()),
            arb_opt(prop::collection::vec(arb_plugin_installation_model(), 0..=2).boxed()),
            arb_opt(arb_vec_merge_mode_model()),
            arb_opt(prop::collection::vec(arb_initial_component_file_model(), 0..=2).boxed()),
        ),
        prop::collection::vec((arb_ident(), arb_component_preset_model()), 0..=2)
            .prop_map(IndexMap::from_iter),
    )
        .prop_map(
            |(
                templates,
                dir,
                component_wasm,
                output_wasm,
                (build_merge_mode, build, custom_commands, clean),
                (config, env_merge_mode, env),
                (plugins_merge_mode, plugins, files_merge_mode, files),
                presets,
            )| Component {
                templates,
                dir,
                component_wasm,
                output_wasm,
                dependencies: ComponentDependencies::default(),
                build_merge_mode,
                build,
                custom_commands,
                clean,
                config,
                initial_card: None,
                env_merge_mode,
                env,
                plugins_merge_mode,
                plugins,
                files_merge_mode,
                files,
                presets,
            },
        )
        .boxed()
}

fn arb_agent_preset_model() -> BoxedStrategy<AgentPreset> {
    (
        any::<bool>(),
        arb_opt(arb_json_value()),
        arb_opt(arb_map_merge_mode_model()),
        arb_opt(arb_string_index_map_model()),
        arb_opt(arb_vec_merge_mode_model()),
        arb_opt(prop::collection::vec(arb_plugin_installation_model(), 0..=2).boxed()),
        arb_opt(arb_vec_merge_mode_model()),
        arb_opt(prop::collection::vec(arb_initial_component_file_model(), 0..=2).boxed()),
        (
            arb_opt(arb_map_merge_mode_model()),
            arb_opt(arb_tool_bindings_model()),
        ),
    )
        .prop_map(
            |(
                is_default,
                config,
                env_merge_mode,
                env,
                plugins_merge_mode,
                plugins,
                files_merge_mode,
                files,
                (tools_merge_mode, tools),
            )| AgentPreset {
                default: is_default.then_some(Marker),
                config,
                initial_card: None,
                env_merge_mode,
                env,
                plugins_merge_mode,
                plugins,
                files_merge_mode,
                files,
                tools_merge_mode,
                tools,
            },
        )
        .boxed()
}

fn arb_agent_model() -> BoxedStrategy<Agent> {
    (
        arb_token_list_model(),
        arb_opt(arb_json_value()),
        arb_opt(arb_map_merge_mode_model()),
        arb_opt(arb_string_index_map_model()),
        arb_opt(arb_vec_merge_mode_model()),
        arb_opt(prop::collection::vec(arb_plugin_installation_model(), 0..=2).boxed()),
        arb_opt(arb_vec_merge_mode_model()),
        arb_opt(prop::collection::vec(arb_initial_component_file_model(), 0..=2).boxed()),
        (
            arb_opt(arb_map_merge_mode_model()),
            arb_opt(arb_tool_bindings_model()),
        ),
        prop::collection::vec((arb_ident(), arb_agent_preset_model()), 0..=2)
            .prop_map(IndexMap::from_iter),
    )
        .prop_map(
            |(
                templates,
                config,
                env_merge_mode,
                env,
                plugins_merge_mode,
                plugins,
                files_merge_mode,
                files,
                (tools_merge_mode, tools),
                presets,
            )| Agent {
                templates,
                config,
                initial_card: None,
                env_merge_mode,
                env,
                plugins_merge_mode,
                plugins,
                files_merge_mode,
                files,
                tools_merge_mode,
                tools,
                presets,
            },
        )
        .boxed()
}

fn arb_server_model() -> BoxedStrategy<Server> {
    prop_oneof![
        Just(Server::Builtin(BuiltinServer::Local)),
        Just(Server::Builtin(BuiltinServer::Cloud)),
        (
            arb_url_model(),
            arb_url_model(),
            any::<bool>(),
            any::<bool>(),
            arb_ident(),
        )
            .prop_map(
                |(url, worker_url, allow_insecure, use_oauth, static_token)| {
                    let auth = if use_oauth {
                        CustomServerAuth::OAuth2 { oauth2: Marker }
                    } else {
                        CustomServerAuth::Static { static_token }
                    };

                    Server::Custom(Box::new(CustomServer {
                        url,
                        worker_url: Some(worker_url),
                        allow_insecure: Some(allow_insecure),
                        auth,
                    }))
                }
            ),
    ]
    .boxed()
}

fn arb_cli_options_model() -> BoxedStrategy<CliOptions> {
    (
        arb_opt(prop_oneof![Just(Format::Text), Just(Format::Json), Just(Format::Toon)].boxed()),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(
            |(format, auto_confirm, redeploy_agents, reset)| CliOptions {
                format,
                auto_confirm: auto_confirm.then_some(Marker),
                redeploy_agents: redeploy_agents.then_some(Marker),
                reset: reset.then_some(Marker),
            },
        )
        .boxed()
}

fn arb_deployment_options_model() -> BoxedStrategy<DeploymentOptions> {
    (any::<bool>(), any::<bool>(), any::<bool>())
        .prop_map(
            |(compatibility_check, version_check, security_overrides)| DeploymentOptions {
                compatibility_check: Some(compatibility_check),
                version_check: Some(version_check),
                security_overrides: Some(security_overrides),
            },
        )
        .boxed()
}

fn arb_git_hash_model() -> BoxedStrategy<GitHashVersionSource> {
    (arb_opt(any::<bool>().boxed()), arb_opt(arb_ident().boxed()))
        .prop_map(|(allow_dirty, static_fallback)| GitHashVersionSource {
            hash_only: Marker,
            allow_dirty,
            static_fallback,
        })
        .boxed()
}

fn arb_app_version_source_model() -> BoxedStrategy<AppVersionSource> {
    let git_tag = (
        arb_ident(),
        arb_opt(any::<bool>().boxed()),
        arb_opt(any::<bool>().boxed()),
        arb_opt(any::<bool>().boxed()),
        arb_opt(arb_ident().boxed()),
    )
        .prop_map(
            |(tag_pattern, commit_info, hash_fallback, allow_dirty, static_fallback)| {
                GitVersionSource::Tag(GitTagVersionSource {
                    tag_pattern,
                    commit_info,
                    hash_fallback,
                    allow_dirty,
                    static_fallback,
                })
            },
        );
    let git_hash = arb_git_hash_model().prop_map(GitVersionSource::Hash);
    prop_oneof![
        prop_oneof![git_tag, git_hash].prop_map(|git| AppVersionSource::Git { git }),
        arb_ident().prop_map(AppVersionSource::Static),
        arb_ident().prop_map(|env| AppVersionSource::Env { env }),
    ]
    .boxed()
}

fn arb_app_version_source_override_model() -> BoxedStrategy<AppVersionSourceOverride> {
    let git_tag = (
        arb_opt(arb_ident().boxed()),
        arb_opt(any::<bool>().boxed()),
        arb_opt(any::<bool>().boxed()),
        arb_opt(any::<bool>().boxed()),
        arb_opt(arb_ident().boxed()),
    )
        .prop_map(
            |(tag_pattern, commit_info, hash_fallback, allow_dirty, static_fallback)| {
                GitVersionSourceOverride::Tag(GitTagVersionSourceOverride {
                    tag_pattern,
                    commit_info,
                    hash_fallback,
                    allow_dirty,
                    static_fallback,
                })
            },
        );
    let git_hash = arb_git_hash_model().prop_map(GitVersionSourceOverride::Hash);
    prop_oneof![
        prop_oneof![git_tag, git_hash].prop_map(|git| AppVersionSourceOverride::Git { git }),
        arb_ident().prop_map(AppVersionSourceOverride::Static),
        arb_ident().prop_map(|env| AppVersionSourceOverride::Env { env }),
    ]
    .boxed()
}

fn arb_environment_model() -> BoxedStrategy<Environment> {
    (
        any::<bool>(),
        arb_opt(arb_ident()),
        arb_opt(arb_server_model()),
        arb_token_list_model(),
        arb_opt(arb_cli_options_model()),
        arb_opt(arb_deployment_options_model()),
        arb_opt(arb_app_version_source_override_model()),
    )
        .prop_map(
            |(is_default, account, server, component_presets, cli, deployment, version)| {
                Environment {
                    default: is_default.then_some(Marker),
                    account,
                    server,
                    component_presets,
                    cli,
                    deployment,
                    version,
                }
            },
        )
        .boxed()
}

fn arb_path_buf_model() -> BoxedStrategy<PathBuf> {
    arb_ident().prop_map(PathBuf::from).boxed()
}

fn arb_local_server_model() -> BoxedStrategy<LocalServer> {
    // Ports must be nonzero to satisfy the manifest schema (min 1).
    (
        arb_opt(
            (1..=u64::MAX)
                .prop_map(|n| std::num::NonZeroU64::new(n).unwrap())
                .boxed(),
        ),
        arb_opt(arb_ident()),
        arb_opt((1..=u16::MAX).boxed()),
        arb_opt((1..=u16::MAX).boxed()),
        arb_opt((1..=u16::MAX).boxed()),
        arb_opt(arb_path_buf_model()),
        arb_opt(arb_path_buf_model()),
        arb_opt(arb_path_buf_model()),
    )
        .prop_map(
            |(
                system_memory_override,
                router_addr,
                router_port,
                custom_request_port,
                mcp_port,
                ports_file,
                data_dir,
                agent_filesystem_root,
            )| LocalServer {
                system_memory_override,
                router_addr,
                router_port,
                custom_request_port,
                mcp_port,
                ports_file,
                data_dir,
                agent_filesystem_root,
            },
        )
        .boxed()
}

fn arb_http_api_deployment_model() -> BoxedStrategy<HttpApiDeployment> {
    (
        arb_dns_label(),
        arb_opt(arb_ident()),
        arb_opt(arb_ident()),
        prop::collection::vec(
            (
                arb_ident().prop_map(AgentTypeName),
                (
                    arb_opt(arb_ident().prop_map(SecuritySchemeName).boxed()),
                    arb_opt(arb_ident()),
                )
                    .prop_map(|(security_scheme, test_session_header_name)| {
                        HttpApiDeploymentAgentOptions {
                            security_scheme,
                            test_session_header_name,
                        }
                    }),
            ),
            0..=3,
        )
        .prop_map(IndexMap::from_iter),
    )
        .prop_map(
            |(domain, webhook_url, openapi_endpoint, agents)| HttpApiDeployment {
                domain: Some(Domain(format!("{domain}.example.com")).into()),
                subdomain: None,
                webhook_url,
                openapi_endpoint,
                agents,
            },
        )
        .boxed()
}

fn arb_http_api_model() -> BoxedStrategy<HttpApi> {
    prop::collection::vec(
        (
            arb_ident().prop_map(EnvironmentName),
            prop::collection::vec(arb_http_api_deployment_model(), 0..=2),
        ),
        0..=3,
    )
    .prop_map(|deployments| HttpApi {
        deployments: IndexMap::from_iter(deployments),
    })
    .boxed()
}

fn arb_mcp_deployment_model() -> BoxedStrategy<McpDeployment> {
    (
        arb_dns_label(),
        prop::collection::vec(
            (
                arb_ident().prop_map(AgentTypeName),
                arb_opt(arb_ident())
                    .prop_map(|security_scheme| McpDeploymentAgentOptions { security_scheme }),
            ),
            0..=3,
        )
        .prop_map(IndexMap::from_iter),
    )
        .prop_map(|(domain, agents)| McpDeployment {
            domain: Some(Domain(format!("{domain}.example.com")).into()),
            subdomain: None,
            agents,
        })
        .boxed()
}

fn arb_mcp_model() -> BoxedStrategy<Mcp> {
    prop::collection::vec(
        (
            arb_ident().prop_map(EnvironmentName),
            prop::collection::vec(arb_mcp_deployment_model(), 0..=2),
        ),
        0..=3,
    )
    .prop_map(|deployments| Mcp {
        deployments: IndexMap::from_iter(deployments),
    })
    .boxed()
}

fn arb_bridge_sdk_language_targets() -> BoxedStrategy<BridgeSdkLanguageTargets> {
    (
        arb_opt(arb_bridge_sdk_external_targets()),
        arb_opt(arb_bridge_sdk_internal_targets()),
    )
        .prop_map(|(external, internal)| BridgeSdkLanguageTargets { external, internal })
        .boxed()
}

fn arb_bridge_sdk_external_targets() -> BoxedStrategy<BridgeSdkExternalTargets> {
    (arb_token_list_model(), arb_opt(arb_ident()))
        .prop_map(|(agents, output_dir)| BridgeSdkExternalTargets { agents, output_dir })
        .boxed()
}

fn arb_bridge_sdk_internal_targets() -> BoxedStrategy<BridgeSdkInternalTargets> {
    (
        arb_token_list_model(),
        arb_token_list_model(),
        arb_opt(arb_ident()),
    )
        .prop_map(|(agents, tools, output_dir)| BridgeSdkInternalTargets {
            agents,
            tools,
            output_dir,
        })
        .boxed()
}

fn arb_bridge_sdks_model() -> BoxedStrategy<BridgeSdks> {
    (
        arb_opt(arb_bridge_sdk_language_targets()),
        arb_opt(arb_bridge_sdk_language_targets()),
        arb_opt(arb_bridge_sdk_language_targets()),
        arb_opt(arb_bridge_sdk_language_targets()),
    )
        .prop_map(|(ts, rust, scala, moonbit)| BridgeSdks {
            ts,
            rust,
            scala,
            moonbit,
        })
        .boxed()
}

fn arb_json_map() -> BoxedStrategy<JsonObject> {
    prop::collection::btree_map(arb_ident(), arb_json_value(), 0..=3)
        .prop_map(|m| m.into_iter().collect())
        .boxed()
}

fn arb_secret_defaults_model() -> BoxedStrategy<IndexMap<EnvironmentName, JsonObject>> {
    prop::collection::vec(
        (arb_ident().prop_map(EnvironmentName), arb_json_map()),
        0..=3,
    )
    .prop_map(IndexMap::from_iter)
    .boxed()
}

fn arb_resource_limit_model() -> BoxedStrategy<ResourceLimit> {
    prop_oneof![
        (1u64..=100u64, 1u64..=300u64).prop_map(|(value, max)| {
            ResourceLimit::Rate(ResourceRateLimit {
                value,
                period: TimePeriod::Second,
                max,
            })
        }),
        (1u64..=100u64).prop_map(|value| ResourceLimit::Capacity(ResourceCapacityLimit { value })),
        (1u64..=100u64)
            .prop_map(|value| ResourceLimit::Concurrency(ResourceConcurrencyLimit { value })),
    ]
    .boxed()
}

fn arb_resource_defaults_model()
-> BoxedStrategy<IndexMap<EnvironmentName, IndexMap<ResourceName, ResourceDefinition>>> {
    prop::collection::vec(
        (
            arb_ident().prop_map(EnvironmentName),
            prop::collection::vec(
                (
                    arb_ident(),
                    arb_resource_limit_model(),
                    any::<bool>(),
                    arb_ident(),
                    arb_ident(),
                )
                    .prop_map(|(name, limit, reject, unit, units)| {
                        (
                            ResourceName(name),
                            ResourceDefinition {
                                limit,
                                enforcement_action: if reject {
                                    EnforcementAction::Reject
                                } else {
                                    EnforcementAction::Throttle
                                },
                                unit,
                                units,
                            },
                        )
                    }),
                0..=2,
            )
            .prop_map(IndexMap::from_iter),
        ),
        0..=3,
    )
    .prop_map(IndexMap::from_iter)
    .boxed()
}

fn arb_api_predicate_value_model() -> BoxedStrategy<ApiPredicateValue> {
    prop_oneof![
        arb_ident().prop_map(|value| ApiPredicateValue::Text(ApiTextValue { value })),
        any::<i64>().prop_map(|value| ApiPredicateValue::Integer(ApiIntegerValue { value })),
        any::<bool>().prop_map(|value| ApiPredicateValue::Boolean(ApiBooleanValue { value })),
    ]
    .boxed()
}

fn arb_api_predicate_model() -> BoxedStrategy<ApiPredicate> {
    let leaf = prop_oneof![
        Just(ApiPredicate::True(ApiPredicateTrue {})),
        Just(ApiPredicate::False(ApiPredicateFalse {})),
        arb_ident()
            .prop_map(|property| { ApiPredicate::PropExists(ApiPropertyExistence { property }) }),
        (arb_ident(), arb_api_predicate_value_model()).prop_map(|(property, value)| {
            ApiPredicate::PropEq(ApiPropertyComparison { property, value })
        }),
        (arb_ident(), arb_api_predicate_value_model()).prop_map(|(property, value)| {
            ApiPredicate::PropNeq(ApiPropertyComparison { property, value })
        }),
        (
            arb_ident(),
            prop::collection::vec(arb_api_predicate_value_model(), 1..=3),
        )
            .prop_map(|(property, values)| {
                ApiPredicate::PropIn(ApiPropertySetCheck { property, values })
            }),
        (arb_ident(), arb_ident()).prop_map(|(property, pattern)| {
            ApiPredicate::PropMatches(ApiPropertyPattern { property, pattern })
        }),
        (arb_ident(), arb_ident()).prop_map(|(property, prefix)| {
            ApiPredicate::PropStartsWith(ApiPropertyPrefix { property, prefix })
        }),
        (arb_ident(), arb_ident()).prop_map(|(property, substring)| {
            ApiPredicate::PropContains(ApiPropertySubstring {
                property,
                substring,
            })
        }),
    ];

    leaf.prop_recursive(3, 48, 2, |inner| {
        prop_oneof![
            (inner.clone(), inner.clone()).prop_map(|(left, right)| {
                ApiPredicate::And(ApiPredicatePair {
                    left: Box::new(left),
                    right: Box::new(right),
                })
            }),
            (inner.clone(), inner.clone()).prop_map(|(left, right)| {
                ApiPredicate::Or(ApiPredicatePair {
                    left: Box::new(left),
                    right: Box::new(right),
                })
            }),
            inner.prop_map(|predicate| {
                ApiPredicate::Not(ApiPredicateNot {
                    predicate: Box::new(predicate),
                })
            }),
        ]
    })
    .boxed()
}

fn arb_api_retry_policy_model() -> BoxedStrategy<ApiRetryPolicy> {
    let leaf = prop_oneof![
        Just(ApiRetryPolicy::Immediate(ApiImmediatePolicy {})),
        Just(ApiRetryPolicy::Never(ApiNeverPolicy {})),
        (0u64..=10000u64)
            .prop_map(|delay_ms| { ApiRetryPolicy::Periodic(ApiPeriodicPolicy { delay_ms }) }),
        (
            (0u64..=10000u64),
            (1u8..=20u8).prop_map(|n| n as f64 / 10.0)
        )
            .prop_map(|(base_delay_ms, factor)| {
                ApiRetryPolicy::Exponential(ApiExponentialPolicy {
                    base_delay_ms,
                    factor,
                })
            },),
        ((0u64..=10000u64), (0u64..=10000u64)).prop_map(|(first_ms, second_ms)| {
            ApiRetryPolicy::Fibonacci(ApiFibonacciPolicy {
                first_ms,
                second_ms,
            })
        }),
    ];

    leaf.prop_recursive(3, 48, 2, |inner| {
        prop_oneof![
            ((0u32..=50u32), inner.clone()).prop_map(|(max_retries, inner)| {
                ApiRetryPolicy::CountBox(ApiCountBoxPolicy {
                    max_retries,
                    inner: Box::new(inner),
                })
            }),
            ((0u64..=100000u64), inner.clone()).prop_map(|(limit_ms, inner)| {
                ApiRetryPolicy::TimeBox(ApiTimeBoxPolicy {
                    limit_ms,
                    inner: Box::new(inner),
                })
            }),
            ((0u64..=100000u64), (0u64..=100000u64), inner.clone()).prop_map(
                |(min_delay_ms, max_delay_ms, inner)| {
                    ApiRetryPolicy::Clamp(ApiClampPolicy {
                        min_delay_ms,
                        max_delay_ms,
                        inner: Box::new(inner),
                    })
                },
            ),
            ((0u64..=100000u64), inner.clone()).prop_map(|(delay_ms, inner)| {
                ApiRetryPolicy::AddDelay(ApiAddDelayPolicy {
                    delay_ms,
                    inner: Box::new(inner),
                })
            }),
            ((1u8..=20u8).prop_map(|n| n as f64 / 10.0), inner.clone()).prop_map(
                |(factor, inner)| {
                    ApiRetryPolicy::Jitter(ApiJitterPolicy {
                        factor,
                        inner: Box::new(inner),
                    })
                },
            ),
            (arb_api_predicate_model(), inner.clone()).prop_map(|(predicate, inner)| {
                ApiRetryPolicy::FilteredOn(ApiFilteredOnPolicy {
                    predicate,
                    inner: Box::new(inner),
                })
            }),
            (inner.clone(), inner.clone()).prop_map(|(first, second)| {
                ApiRetryPolicy::AndThen(ApiRetryPolicyPair {
                    first: Box::new(first),
                    second: Box::new(second),
                })
            }),
            (inner.clone(), inner.clone()).prop_map(|(first, second)| {
                ApiRetryPolicy::Union(ApiRetryPolicyPair {
                    first: Box::new(first),
                    second: Box::new(second),
                })
            }),
            (inner.clone(), inner.clone()).prop_map(|(first, second)| {
                ApiRetryPolicy::Intersect(ApiRetryPolicyPair {
                    first: Box::new(first),
                    second: Box::new(second),
                })
            }),
        ]
    })
    .boxed()
}

fn arb_retry_policy_defaults_model()
-> BoxedStrategy<IndexMap<EnvironmentName, IndexMap<String, EnvironmentRetryPolicy>>> {
    prop::collection::vec(
        (
            arb_ident().prop_map(EnvironmentName),
            prop::collection::vec(
                (
                    arb_ident(),
                    0u32..=100u32,
                    arb_api_predicate_model(),
                    arb_api_retry_policy_model(),
                )
                    .prop_map(|(name, priority, predicate, policy)| {
                        (
                            name,
                            EnvironmentRetryPolicy {
                                priority,
                                predicate: predicate.into(),
                                policy: policy.into(),
                            },
                        )
                    }),
                0..=2,
            )
            .prop_map(IndexMap::from_iter),
        ),
        0..=3,
    )
    .prop_map(IndexMap::from_iter)
    .boxed()
}

fn arb_application_model_v3() -> BoxedStrategy<Application> {
    (
        arb_opt(arb_semver()),
        arb_opt(arb_ident()),
        prop::collection::vec(arb_ident(), 0..=3),
        (
            prop::collection::vec((arb_ident(), arb_component_template_model()), 0..=3)
                .prop_map(IndexMap::from_iter),
            prop::collection::vec((arb_ident(), arb_component_model()), 0..=3)
                .prop_map(IndexMap::from_iter),
            prop::collection::vec(
                (arb_ident().prop_map(AgentTypeName), arb_agent_model()),
                0..=3,
            )
            .prop_map(IndexMap::from_iter),
            prop::collection::vec(
                (
                    arb_ident(),
                    prop::collection::vec(arb_external_command_model(), 0..=2),
                ),
                0..=3,
            )
            .prop_map(IndexMap::from_iter),
            prop::collection::vec(arb_ident(), 0..=3),
            arb_tool_declarations_model(),
        ),
        (
            arb_opt(arb_http_api_model()),
            arb_opt(arb_mcp_model()),
            arb_opt(arb_local_server_model()),
            prop::collection::vec((arb_ident(), arb_environment_model()), 0..=3)
                .prop_map(IndexMap::from_iter),
            arb_opt(arb_app_version_source_model()),
            arb_opt(arb_bridge_sdks_model()),
            arb_secret_defaults_model(),
            arb_retry_policy_defaults_model(),
            arb_resource_defaults_model(),
        ),
    )
        .prop_map(
            |(
                manifest_version,
                app,
                includes,
                (component_templates, components, agents, custom_commands, clean, tools),
                (
                    http_api,
                    mcp,
                    local_server,
                    environments,
                    version,
                    bridge,
                    secret_defaults,
                    retry_policy_defaults,
                    resource_defaults,
                ),
            )| Application {
                manifest_version,
                app,
                includes,
                component_templates,
                components,
                agents,
                tools,
                custom_commands,
                clean,
                http_api,
                mcp,
                local_server,
                environments,
                version,
                bridge,
                secret_defaults,
                retry_policy_defaults,
                resource_defaults,
                tool_releases: Default::default(),
            },
        )
        .boxed()
}

prop_compose! {
    fn arb_application_document()(app in arb_application_model_v3()) -> Application {
        app
    }
}

#[test]
fn schema_is_loadable_and_validates_empty_app() {
    let app = Application {
        app: Some("app-name".to_string()),
        ..Default::default()
    };

    assert!(JSON_SCHEMA_VALIDATOR.is_valid(&serde_json::to_value(&app).unwrap()));
}

#[test]
fn schema_and_serde_accept_slice_b_tool_manifest_fields() {
    let source = indoc::indoc! { r#"
            app: test-app
            environments:
              local:
                server: local
            toolReleases:
              local:
                grep: {}
            components:
              app:main:
                componentWasm: main.wasm
            agents:
              CoderAgent:
                tools:
                  grep:
                    version: "1.0.0"
                    parametersMergeMode: replace
                    parameters: { root: /workspace/src }
                    configKeysReadableMergeMode: intersect
                    configKeysReadable: [runtime.logLevel]
                    secretKeysReadableMergeMode: intersect
                    secretKeysReadable: [credentials.github]
                    secretKeysRevealable: []
                presets:
                  debug:
                    toolsMergeMode: remove
                    tools:
                      grep: {}
            tools:
              grep:
                component: app:main
                templates: rust
                config: { logLevel: info }
                env: { RUST_LOG: info }
                presets:
                  app-env:local:
                    config: { logLevel: warn }
                  debug:
                    default: true
                    files:
                      - sourcePath: tool.txt
                        targetPath: /tool.txt
        "# };

    let app = Application::from_yaml_str(source).unwrap();
    let value = serde_yaml::from_str::<serde_json::Value>(source).unwrap();

    assert!(JSON_SCHEMA_VALIDATOR.is_valid(&value));
    assert!(!app.tools.is_empty());
    assert_eq!(app.tool_releases.len(), 1);
}

#[test]
fn tool_binding_rejects_unknown_config_scope_field() {
    let source = indoc::indoc! { r#"
            app: test-app
            agents:
              CoderAgent:
                tools:
                  grep:
                    configKeysReadble: "*"
        "# };

    assert!(Application::from_yaml_str(source).is_err());
}

#[test]
fn schema_rejects_wildcard_inside_secret_scope_list() {
    let value = serde_json::json!({
        "app": "test-app",
        "environments": {
            "local": {
                "server": "local"
            }
        },
        "agents": {
            "CoderAgent": {
                "tools": {
                    "grep": {
                        "secretKeysReadable": ["*"]
                    }
                }
            }
        }
    });

    assert!(!JSON_SCHEMA_VALIDATOR.is_valid(&value));
}

#[test]
fn manifest_loading_rejects_invalid_tool_binding_scopes() {
    for (field, value) in [
        ("configKeysReadable", "anything"),
        ("configKeysReadable", "['*']"),
        ("secretKeysReadable", "anything"),
        ("secretKeysRevealable", "['*']"),
        ("secretKeysReadable", "['credentials\\']"),
    ] {
        let source = format!(
            "app: test-app\nagents:\n  CoderAgent:\n    tools:\n      grep:\n        {field}: {value}\n"
        );
        assert!(
            Application::from_yaml_str(&source).is_err(),
            "manifest unexpectedly accepted {field}: {value}"
        );
    }
}

#[test]
fn manifest_loading_accepts_wildcard_and_escaped_tool_binding_paths() {
    let source = indoc::indoc! { r#"
            app: test-app
            agents:
              CoderAgent:
                tools:
                  grep:
                    configKeysReadable: "*"
                    secretKeysReadable: ['credentials.github\=token']
                    secretKeysRevealable: ['"database url".password']
        "# };

    Application::from_yaml_str(source).expect("valid scopes should load");
}

#[test]
fn manifest_loading_validates_tool_names_in_every_agent_layer() {
    for body in [
        "tools:\n      Invalid_Name: {}",
        "presets:\n      unselected:\n        tools:\n          Invalid_Name: {}",
        "presets:\n      selected-by-default:\n        default: true\n        tools:\n          Invalid_Name: {}",
    ] {
        let source = format!("app: test-app\nagents:\n  CoderAgent:\n    {body}\n");
        assert!(
            Application::from_yaml_str(&source).is_err(),
            "manifest unexpectedly accepted invalid tool name in:\n{body}"
        );
    }
}

#[test]
fn environment_rejects_tool_bindings_and_publications() {
    for field in ["tools", "toolsMergeMode", "publishTools"] {
        let source = format!(
            "app: test-app\nenvironments:\n  local:\n    server: local\n    {field}: {{}}\n"
        );

        assert!(
            Application::from_yaml_str(&source).is_err(),
            "environment unexpectedly accepted {field}"
        );
    }
}

#[test]
fn tool_declaration_accepts_literal_release_and_rejects_old_source_shape() {
    let declaration = serde_yaml::from_str::<ToolDeclaration>(indoc::indoc! { r#"
            release:
              account: publisher@example.com
              name: grep
              version: "{{ VERSION }}"
        "# })
    .expect("literal release coordinates should parse");

    assert!(matches!(
        declaration.release,
        Some(RegistrySubject::ByCoordinates(RegistrySubjectByCoordinates { version, .. }))
            if version == "{{ VERSION }}"
    ));
    assert!(
        serde_yaml::from_str::<ToolDeclaration>(
            "source:\n  registry:\n    releaseId: 00000000-0000-0000-0000-000000000001\n"
        )
        .is_err()
    );
}

#[test]
fn registry_subject_reports_expected_fields_for_typos() {
    let error = serde_yaml::from_str::<ToolDeclaration>(
        "release:\n  account: publisher@example.com\n  name: grep\n  vesrion: 1.2.3\n",
    )
    .unwrap_err()
    .to_string();

    assert!(
        error.contains("expected `account`, `name`, and `version`")
            && error.contains("unknown field `vesrion`"),
        "unexpected error: {error}"
    );
}

#[test]
fn bridge_rust_agents_keeps_parsing_as_external_bridge_targets() {
    let source = indoc::indoc! { r#"
            app: test-app

            bridge:
              rust:
                external:
                  agents: CounterAgent
                  outputDir: bridge/rust
        "# };

    let app = Application::from_yaml_str(source).unwrap();
    let rust = app.bridge.unwrap().rust.unwrap();
    let external = rust.external.unwrap();

    assert_eq!(external.agents.into_vec(), vec!["CounterAgent".to_string()]);
    assert_eq!(external.output_dir.as_deref(), Some("bridge/rust"));
    assert!(rust.internal.is_none());
}

#[test]
fn bridge_rust_guest_parses_as_guest_bridge_targets() {
    let source = indoc::indoc! { r#"
            app: test-app

            bridge:
              rust:
                external:
                  agents: ExternalAgent
                  outputDir: bridge/rust
                internal:
                  agents:
                    - GuestAgent
                  outputDir: bridge/rust-guest
        "# };

    let app = Application::from_yaml_str(source).unwrap();
    let rust = app.bridge.unwrap().rust.unwrap();
    let external = rust.external.unwrap();
    let guest = rust.internal.unwrap();

    assert_eq!(
        external.agents.into_vec(),
        vec!["ExternalAgent".to_string()]
    );
    assert_eq!(external.output_dir.as_deref(), Some("bridge/rust"));
    assert_eq!(guest.agents.into_vec(), vec!["GuestAgent".to_string()]);
    assert_eq!(guest.output_dir.as_deref(), Some("bridge/rust-guest"));
}

#[test]
fn root_version_source_shape_rules() {
    fn parse(yaml: &str) -> Result<AppVersionSource, serde_yaml::Error> {
        serde_yaml::from_str(yaml)
    }
    // bare string is a literal version, and env is a map variant
    assert!(matches!(parse("\"1.2.3\"").unwrap(), AppVersionSource::Static(v) if v == "1.2.3"));
    assert!(matches!(
        parse("env: MY_VERSION").unwrap(),
        AppVersionSource::Env { .. }
    ));
    assert!(matches!(
        parse("git:\n  hashOnly: true").unwrap(),
        AppVersionSource::Git {
            git: GitVersionSource::Hash(_)
        }
    ));
    // tag mode requires an explicit tagPattern
    assert!(matches!(
        parse("git:\n  tagPattern: \"v*\"").unwrap(),
        AppVersionSource::Git {
            git: GitVersionSource::Tag(_)
        }
    ));
    // root tag mode without tagPattern is rejected (no default), as is empty git
    assert!(parse("git:\n  hashFallback: true").is_err());
    assert!(parse("git: {}").is_err());
    // mixing hash and tag options is rejected
    assert!(parse("git:\n  hashOnly: true\n  tagPattern: \"v*\"").is_err());
    // hashOnly must be true; unknown fields rejected
    assert!(parse("git:\n  hashOnly: false").is_err());
    assert!(parse("git:\n  tagPattern: \"v*\"\n  bogus: 1").is_err());
}

#[test]
fn override_version_source_shape_rules() {
    fn parse(yaml: &str) -> Result<AppVersionSourceOverride, serde_yaml::Error> {
        serde_yaml::from_str(yaml)
    }
    assert!(
        matches!(parse("\"9.9.9\"").unwrap(), AppVersionSourceOverride::Static(v) if v == "9.9.9")
    );
    // partial tag override without tagPattern is allowed
    assert!(matches!(
        parse("git:\n  allowDirty: true").unwrap(),
        AppVersionSourceOverride::Git {
            git: GitVersionSourceOverride::Tag(_)
        }
    ));
    assert!(matches!(
        parse("git: {}").unwrap(),
        AppVersionSourceOverride::Git {
            git: GitVersionSourceOverride::Tag(_)
        }
    ));
    assert!(matches!(
        parse("git:\n  hashOnly: true").unwrap(),
        AppVersionSourceOverride::Git {
            git: GitVersionSourceOverride::Hash(_)
        }
    ));
    // mixing hash and tag options is still rejected
    assert!(parse("git:\n  hashOnly: true\n  tagPattern: \"v*\"").is_err());
}

#[test]
fn secret_defaults_accepts_object_per_environment() {
    let source = indoc::indoc! { r#"
            app: test-app

            secretDefaults:
              local:
                db:
                  password: secret
        "# };

    let result = Application::from_yaml_str(source);

    assert!(result.is_ok(), "{:?}", result.err());
}

#[test]
fn agents_accept_initial_card() {
    let source = indoc::indoc! { r#"
            app: test-app

            agents:
              Cart:
                initialCard:
                  lowerBound:
                    positive:
                      - 'filesystem(?self) @ account@example.com/app/env/component/Cart : read : /data/**'
                    negative: []
                  upperBound:
                    positive: []
                    negative: []
        "# };

    let app = Application::from_yaml_str(source).expect("initialCard should parse");
    let initial_card = app
        .agents
        .get(&AgentTypeName("Cart".to_string()))
        .and_then(|agent| agent.initial_card.as_ref())
        .expect("agent initialCard should be present");

    assert_eq!(initial_card.lower_bound.positive.len(), 1);
}

#[test]
fn secret_defaults_rejects_scalar_environment_value() {
    let source = indoc::indoc! { r#"
            app: test-app

            secretDefaults:
              local: secret
        "# };

    let result = Application::from_yaml_str(source);

    let err = result.expect_err("secretDefaults.local should require an object");
    let err = err.to_string();
    assert!(
        err.contains("secretDefaults.local") || err.contains("secret_defaults.local"),
        "unexpected error: {err}"
    );
}

#[test]
fn secret_defaults_rejects_array_environment_value() {
    let source = indoc::indoc! { r#"
            app: test-app

            secretDefaults:
              local:
                - secret
        "# };

    let result = Application::from_yaml_str(source);

    let err = result.expect_err("secretDefaults.local should require an object");
    let err = err.to_string();
    assert!(
        err.contains("secretDefaults.local") || err.contains("secret_defaults.local"),
        "unexpected error: {err}"
    );
}

#[test]
fn local_server_accepts_server_run_defaults() {
    let source = indoc::indoc! { r#"
            app: test-app

            localServer:
              systemMemoryOverride: 2 GiB
              routerAddr: 127.0.0.1
              routerPort: 9882
              customRequestPort: 9008
              mcpPort: 9009
              portsFile: .golem/ports.json
              dataDir: .golem/server-data
              agentFilesystemRoot: .golem/agents
        "# };

    let app = Application::from_yaml_str(source).unwrap();
    let local_server = app.local_server.expect("localServer should be parsed");

    assert_eq!(
        local_server.system_memory_override.unwrap().get(),
        2147483648
    );
    assert_eq!(local_server.router_addr.as_deref(), Some("127.0.0.1"));
    assert_eq!(local_server.router_port, Some(9882));
    assert_eq!(local_server.custom_request_port, Some(9008));
    assert_eq!(local_server.mcp_port, Some(9009));
    assert_eq!(
        local_server.ports_file,
        Some(PathBuf::from(".golem/ports.json"))
    );
    assert_eq!(
        local_server.data_dir,
        Some(PathBuf::from(".golem/server-data"))
    );
    assert_eq!(
        local_server.agent_filesystem_root,
        Some(PathBuf::from(".golem/agents"))
    );
}

#[test]
fn local_server_system_memory_override_rejects_invalid_values() {
    for value in [
        "'0 B'",
        "'-1 MB'",
        "'18446744073709551616 B'",
        "'garbage'",
        "2147483648",
    ] {
        let source = format!("app: test-app\nlocalServer:\n  systemMemoryOverride: {value}\n");
        assert!(
            Application::from_yaml_str(&source).is_err(),
            "accepted {value}"
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 400,
        .. ProptestConfig::default()
    })]

    #[test]
    fn proptest_schema_accepts_serialized_application_documents(app in arb_application_document()) {
        let json_str = serde_json::to_string(&app).unwrap();
        let yaml_str = serde_yaml::to_string(&app).unwrap();

        let app_from_json: Application = serde_json::from_str(&json_str).unwrap();
        let app_from_yaml: Application = serde_yaml::from_str(&yaml_str).unwrap();

        let app_from_json_value = serde_json::to_value(&app_from_json).unwrap();
        let app_from_yaml_value = serde_json::to_value(&app_from_yaml).unwrap();
        let app_original_value = serde_json::to_value(&app).unwrap();

        prop_assert_eq!(app_from_json_value.clone(), app_from_yaml_value);
        prop_assert_eq!(app_from_json_value, app_original_value);

        let json_value: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        let json_evaluation = JSON_SCHEMA_VALIDATOR.evaluate(&json_value);
        prop_assert!(
            json_evaluation.flag().valid,
            "Schema validation failed for generated app JSON payload: {:?}",
            json_evaluation
                .iter_errors()
                .map(|e| format!("{} :: {}", e.instance_location, e.error))
                .collect::<Vec<_>>()
        );

        let yaml_value: serde_json::Value = serde_yaml::from_str(&yaml_str).unwrap();
        let yaml_evaluation = JSON_SCHEMA_VALIDATOR.evaluate(&yaml_value);
        prop_assert!(
            yaml_evaluation.flag().valid,
            "Schema validation failed for generated app YAML payload: {:?}",
            yaml_evaluation
                .iter_errors()
                .map(|e| format!("{} :: {}", e.instance_location, e.error))
                .collect::<Vec<_>>()
        );
    }
}
