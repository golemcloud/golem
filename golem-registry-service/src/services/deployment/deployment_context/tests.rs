use super::*;
use crate::repo::model::deployment::{CompiledMcpData, DeploymentCompiledMcpRecord};
use golem_common::model::Empty;
use golem_common::model::account::{AccountEmail, AccountId, AccountSummary};
use golem_common::model::agent::{AgentMode, Snapshotting};
use golem_common::model::agent_secret::{AgentSecretId, AgentSecretPath, AgentSecretRevision};
use golem_common::model::application::{ApplicationId, ApplicationName};
use golem_common::model::component::{ComponentId, ComponentName, ComponentRevision};
use golem_common::model::component_metadata::{ComponentMetadata, KnownExports};
use golem_common::model::environment::{EnvironmentId, EnvironmentName, EnvironmentRevision};
use golem_common::model::json::NormalizedJsonValue;
use golem_common::model::mcp_deployment::{
    McpDeployment, McpDeploymentAgentOptions, McpDeploymentId, McpDeploymentRevision,
    McpDeploymentToolOptions,
};
use golem_common::model::tool::{RemoteToolDeployment, SecretKeyScope, ToolProvisionConfig};
use golem_common::model::tool_middleware::ToolMiddlewareMergeMode;
use golem_common::model::tool_release::{
    ToolRelease, ToolReleaseById, ToolReleaseId, ToolReleaseLifecycle, ToolReleaseOrigin,
    ToolReleaseReference,
};
use golem_common::schema::agent::{
    AgentConfigDeclarationSchema, AgentConstructorSchema, InputSchema,
};
use golem_common::schema::graph::SchemaTypeDef;
use golem_common::schema::metadata::TypeId;
use golem_common::schema::schema_type::{QuotaTokenSpec, SchemaType, SecretSpec};
use golem_common::schema::schema_value::SchemaValue;
use golem_common::schema::tool::{
    CommandBody, CommandIndex, CommandNode, CommandTree, Doc, Globals, Positionals, Tool,
};
use golem_service_base::mcp::CompiledMcp;
use golem_service_base::repo::Blob;
use serde_json::json;
use std::collections::BTreeSet;
use test_r::test;

fn test_environment() -> Environment {
    Environment {
        id: EnvironmentId::new(),
        revision: EnvironmentRevision::INITIAL,
        application_id: ApplicationId::new(),
        application_name: ApplicationName::try_from("app").unwrap(),
        name: EnvironmentName::try_from("dev").unwrap(),
        diff_model_version: 0,
        compatibility_check: false,
        tool_compatibility_mode: Default::default(),
        version_check: false,
        security_overrides: false,
        owner_account_id: AccountId::new(),
        owner_account_email: AccountEmail::new("owner@example.com"),
        current_deployment: None,
    }
}

fn test_implementer() -> RegisteredAgentTypeImplementer {
    RegisteredAgentTypeImplementer {
        component_id: ComponentId::new(),
        component_revision: ComponentRevision::INITIAL,
        component_name: "component".to_string(),
        account_id: AccountId::new(),
        account_email: AccountEmail::new("owner@example.com"),
    }
}

fn http_context(agents: Vec<AgentTypeSchema>) -> DeploymentContext {
    use golem_common::model::http_api_deployment::{
        HttpApiDeploymentId, HttpApiDeploymentRevision,
    };
    let environment = test_environment();
    let domain = Domain("example.com".into());
    let deployment = HttpApiDeployment {
        scheme: Default::default(),
        id: HttpApiDeploymentId::new(),
        revision: HttpApiDeploymentRevision::INITIAL,
        environment_id: environment.id,
        domain: domain.clone(),
        hash: diff::Hash::empty(),
        agents: agents
            .iter()
            .map(|agent| (agent.type_name.clone(), Default::default()))
            .collect(),
        webhooks_prefix: "/webhooks".into(),
        openapi_endpoint_prefix: "/".into(),
        created_at: chrono::Utc::now(),
    };
    DeploymentContext {
        environment,
        components: BTreeMap::new(),
        http_api_deployments: BTreeMap::from([(domain, deployment)]),
        mcp_deployments: BTreeMap::new(),
        registered_agent_types: agents
            .into_iter()
            .map(|agent| {
                (
                    agent.type_name.clone(),
                    InProgressDeployedRegisteredAgentType {
                        agent_type: agent,
                        implemented_by: test_implementer(),
                        webhook_domain_and_segments: None,
                    },
                )
            })
            .collect(),
    }
}

fn router_agent(name: &str, path: &str) -> AgentTypeSchema {
    use golem_common::model::agent::{CorsOptions, HttpMountDetails, LiteralSegment, PathSegment};
    let mut agent = agent_type_with_secret_config(
        AgentTypeName(name.into()),
        vec!["key".into()],
        SchemaType::string(),
    );
    agent.kind = golem_common::schema::AgentTypeKind::HttpRouter;
    agent.mode = AgentMode::Ephemeral;
    agent.http_mount = Some(HttpMountDetails {
        path_prefix: path
            .split('/')
            .filter(|part| !part.is_empty())
            .map(|value| {
                PathSegment::Literal(LiteralSegment {
                    value: value.into(),
                })
            })
            .collect(),
        auth_details: None,
        phantom_agent: false,
        cors_options: CorsOptions {
            allowed_patterns: vec![],
        },
        webhook_suffix: vec![],
        static_bindings: vec![],
        filesystem_bindings: vec![],
        openapi_provider_method: None,
    });
    agent
}

fn provider_method(name: &str) -> golem_common::schema::AgentMethodSchema {
    golem_common::schema::AgentMethodSchema {
        name: name.into(),
        description: String::new(),
        prompt_hint: None,
        input_schema: InputSchema::Parameters(vec![]),
        output_schema: golem_common::schema::OutputSchema::Single(Box::new(SchemaType::string())),
        http_endpoint: vec![],
        read_only: None,
    }
}

#[test]
fn http_mount_compilation_loads_metadata_corpus() {
    use golem_common::model::agent::FileMapping;
    use golem_service_base::custom_api::RouteBehaviour;
    let corpus: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../golem-service-base/tests/fixtures/http-handlers/corpus.json"
    ))
    .unwrap();
    let ids = [
        "metadata-static-only",
        "metadata-provider-only",
        "metadata-durable-router",
        "metadata-parameterized-router",
    ];
    for id in ids {
        let case = corpus["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["id"] == id)
            .unwrap();
        let input = &case["input"];
        let mut agent = router_agent("site", input["mounts"][0].as_str().unwrap());
        if input["mode"] == "durable" {
            agent.mode = AgentMode::Durable;
        }
        agent.constructor.input_schema = InputSchema::Parameters(
            input["constructor"]
                .as_array()
                .unwrap()
                .iter()
                .map(|field| {
                    golem_common::schema::NamedField::user_supplied(
                        field[0].as_str().unwrap(),
                        SchemaType::string(),
                    )
                })
                .collect(),
        );
        if let Some(mappings) = input["static_bindings"].as_array() {
            agent.http_mount.as_mut().unwrap().static_bindings = FileMapping::compile_list(
                mappings
                    .iter()
                    .map(|mapping| (mapping[0].as_str().unwrap(), mapping[1].as_str().unwrap())),
            )
            .unwrap();
        }
        if let Some(provider) = input["provider"].as_str() {
            agent.http_mount.as_mut().unwrap().openapi_provider_method = Some(provider.into());
            agent.methods = vec![provider_method(provider)];
        }
        let context = http_context(vec![agent]);
        let mut errors = vec![];
        let routes = context.compile_http_api_routes(&HashMap::new(), &mut errors, &mut vec![]);
        if let Some(expected_error) = case["expect"]["error"].as_str() {
            let expected_message = match expected_error {
                "router-mode" => "HTTP router agents must be ephemeral",
                "router-constructor" => "HTTP router constructors cannot have input parameters",
                other => panic!("{id}: unhandled validation category {other}"),
            };
            assert!(
                errors
                    .iter()
                    .any(|error| error.to_string().contains(expected_message)),
                "{id}: {errors:?}"
            );
        } else {
            assert!(errors.is_empty(), "{id}: {errors:?}");
            let RouteBehaviour::HttpRouter(router) = &routes[0].behaviour else {
                panic!("{id}: expected router")
            };
            assert_eq!(
                router
                    .handler
                    .as_ref()
                    .map(|method| method.method_name.as_str()),
                case["expect"]["handler"].as_str(),
                "{id}"
            );
            assert_eq!(
                router
                    .openapi_provider_method
                    .as_ref()
                    .map(|method| method.method_name.as_str()),
                case["expect"]["provider"].as_str(),
                "{id}"
            );
            let bytes = desert_rust::serialize_to_byte_vec(&routes[0]).unwrap();
            let reloaded: UnboundCompiledRoute = desert_rust::deserialize(&bytes).unwrap();
            reloaded
                .route_match
                .validate(&reloaded.path, &reloaded.behaviour)
                .unwrap();
        }
    }
}

#[test]
async fn router_index_preparation_reuses_upload_hash_and_pins_policy() {
    use super::super::router_file_index::prepare_router_file_indexes;
    use golem_common::model::component::{AgentFilePermissions, InitialAgentFile};
    use golem_common::model::component_metadata::AgentTypeProvisionConfig;
    use golem_common::model::path::AgentFilePath;
    use golem_service_base::custom_api::RouteBehaviour;
    use golem_service_base::replayable_stream::ReplayableStream;
    use golem_service_base::service::initial_agent_files::InitialAgentFilesService;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use std::sync::Arc;
    let files = Arc::new(InitialAgentFilesService::new(Arc::new(
        InMemoryBlobStorage::new(),
    )));
    let mut context = http_context(vec![router_agent("selected", "/")]);
    let key = files
        .put_if_not_exists(
            context.environment.id,
            b"abc"
                .to_vec()
                .map_item(|item| item.map_err(anyhow::Error::from))
                .map_error(anyhow::Error::from),
        )
        .await
        .unwrap();
    let file = InitialAgentFile {
        path: AgentFilePath::from_abs_str("/public/a").unwrap(),
        content_hash: key,
        permissions: AgentFilePermissions::ReadOnly,
        size: 3,
    };
    let provision = |file| AgentTypeProvisionConfig {
        files: vec![file],
        env: BTreeMap::new(),
        config: vec![],
        plugins: vec![],
        initial_permissions: golem_common::model::card::PolymorphicCard {
            card_id: golem_common::model::card::CardId::new(),
            parent_ids: vec![],
            lower_positive: vec![],
            lower_negative: vec![],
            upper_positive: vec![],
            upper_negative: vec![],
            created_at: chrono::Utc::now(),
            expires_at: None,
            system_card: false,
        },
    };
    let mut component = test_tool_component("component", BTreeMap::new());
    component.environment_id = context.environment.id;
    let implementer = &mut context
        .registered_agent_types
        .get_mut(&AgentTypeName("selected".into()))
        .unwrap()
        .implemented_by;
    implementer.component_id = component.id;
    implementer.component_revision = component.revision;
    let mut other_file = file.clone();
    other_file.path = AgentFilePath::from_abs_str("/private/other").unwrap();
    component.metadata = ComponentMetadata::default().with_provision_configs(BTreeMap::from([
        (AgentTypeName("selected".into()), provision(file)),
        (AgentTypeName("other".into()), provision(other_file)),
    ]));
    context
        .components
        .insert(component.component_name.clone(), component.clone());
    let mut errors = vec![];
    let mut routes = context.compile_http_api_routes(&HashMap::new(), &mut errors, &mut vec![]);
    assert!(errors.is_empty(), "{errors:?}");
    drop(files);
    prepare_router_file_indexes(&context, &mut routes).unwrap();
    let RouteBehaviour::HttpRouter(router) = &routes[0].behaviour else {
        unreachable!()
    };
    assert_eq!(
        router
            .file_index
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        vec!["/public/a"]
    );
    assert_eq!(router.component_revision, component.revision);
    assert_eq!(router.file_index[0].size, 3);
    assert_eq!(
        router.file_index[0]
            .blob_key
            .0
            .as_blake3_hash()
            .to_hex()
            .as_str(),
        "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"
    );

    let bytes = desert_rust::serialize_to_byte_vec(&routes).unwrap();
    let reloaded: Vec<UnboundCompiledRoute> = desert_rust::deserialize(&bytes).unwrap();
    let RouteBehaviour::HttpRouter(router) = &reloaded[0].behaviour else {
        unreachable!()
    };
    assert_eq!(router.file_index[0].blob_key, key);
    assert_eq!(router.file_index[0].size, 3);
    context
        .components
        .get_mut(&component.component_name)
        .unwrap()
        .revision = ComponentRevision::try_from(99u64).unwrap();
    assert!(prepare_router_file_indexes(&context, &mut routes).is_err());
    let mut provisions = component.metadata.agent_type_provision_configs().clone();
    provisions
        .get_mut(&AgentTypeName("selected".into()))
        .unwrap()
        .files[0]
        .path = AgentFilePath::from_abs_str("/public/bad\\name").unwrap();
    component.metadata = component.metadata.with_provision_configs(provisions);
    context
        .components
        .insert(component.component_name.clone(), component);
    assert!(matches!(
        prepare_router_file_indexes(&context, &mut routes),
        Err(DeploymentWriteError::DeploymentValidationFailed(_))
    ));
}

#[test]
fn http_mount_compilation_provider_limit_and_order() {
    let agents = (0..65)
        .map(|index| {
            let mut agent = router_agent(
                &format!("site{index:02}"),
                &format!("/mount{:02}", 64 - index),
            );
            agent.http_mount.as_mut().unwrap().openapi_provider_method = Some("describe".into());
            agent.methods.push(provider_method("describe"));
            agent
        })
        .collect::<Vec<_>>();
    for count in [64, 65] {
        let context = http_context(agents[..count].to_vec());
        let mut errors = vec![];
        let routes = context.compile_http_api_routes(&HashMap::new(), &mut errors, &mut vec![]);
        assert_eq!(errors.is_empty(), count == 64, "{errors:?}");
        if count == 64 {
            assert!(
                routes
                    .windows(2)
                    .all(|routes| routes[0].route_id < routes[1].route_id)
            );
            assert_eq!(routes[0].path[0].literal_value(), Some("mount01"));
            assert_eq!(routes[63].path[0].literal_value(), Some("mount64"));
        }
    }
    let context = http_context(vec![router_agent("a", "/a/b"), router_agent("z", "/a-")]);
    let mut errors = vec![];
    let routes = context.compile_http_api_routes(&HashMap::new(), &mut errors, &mut vec![]);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(routes[0].path[0].literal_value(), Some("a-"));
    assert!(routes[0].route_id < routes[1].route_id);
}

#[test]
fn http_mount_compilation_rejects_equal_not_nested_or_literal_parameter_overlap() {
    use golem_common::model::agent::{FileMapping, PathSegment, PathVariable};
    let live = |name: &str| {
        let mut agent = router_agent(name, "/site");
        agent.kind = golem_common::schema::AgentTypeKind::Regular;
        agent.mode = AgentMode::Durable;
        let mount = agent.http_mount.as_mut().unwrap();
        mount
            .path_prefix
            .push(PathSegment::PathVariable(PathVariable {
                variable_name: "id".into(),
            }));
        mount.filesystem_bindings = vec![FileMapping::compile("/*", "/public/$1").unwrap()];
        agent.constructor.input_schema =
            InputSchema::Parameters(vec![golem_common::schema::NamedField::user_supplied(
                "id",
                SchemaType::string(),
            )]);
        agent
    };
    for (agents, accepted) in [
        (
            vec![router_agent("a", "/site"), router_agent("b", "/site")],
            false,
        ),
        (vec![live("a"), live("b")], false),
        (
            vec![router_agent("a", "/site"), router_agent("b", "/site/child")],
            true,
        ),
        (vec![live("a"), router_agent("b", "/site/fixed")], true),
    ] {
        let context = http_context(agents);
        let mut errors = vec![];
        context.compile_http_api_routes(&HashMap::new(), &mut errors, &mut vec![]);
        assert_eq!(errors.is_empty(), accepted, "{errors:?}");
    }
}

#[test]
fn http_mount_compilation_typed_filesystem_overlap_and_reserved_bindings() {
    use golem_common::model::agent::{CorsOptions, FileMapping, HttpEndpointDetails, HttpMethod};
    use golem_service_base::custom_api::{PathSegment, RouteBehaviour};
    let mut agent = router_agent("site", "/site");
    agent.kind = golem_common::schema::AgentTypeKind::Regular;
    agent.mode = AgentMode::Durable;
    agent.http_mount.as_mut().unwrap().filesystem_bindings =
        vec![FileMapping::compile("/*", "/public/$1").unwrap()];
    let mut method = provider_method("typed");
    method.http_endpoint = vec![HttpEndpointDetails {
        http_method: HttpMethod::Get(Empty {}),
        path_suffix: vec![],
        header_vars: vec![],
        query_vars: vec![],
        auth_details: None,
        cors_options: CorsOptions {
            allowed_patterns: vec![],
        },
    }];
    agent.methods = vec![method];
    let context = http_context(vec![agent]);
    let mut errors = vec![];
    let routes = context.compile_http_api_routes(&HashMap::new(), &mut errors, &mut vec![]);
    assert!(errors.is_empty(), "{errors:?}");
    assert!(
        routes
            .iter()
            .any(|route| matches!(route.behaviour, RouteBehaviour::AgentFilesystem(_)))
    );
    assert!(
        routes
            .iter()
            .any(|route| matches!(route.behaviour, RouteBehaviour::CallAgent(_)))
    );
    let encoded = desert_rust::serialize_to_byte_vec(&routes).unwrap();
    let typed = routes
        .iter()
        .find(|route| matches!(route.behaviour, RouteBehaviour::CallAgent(_)))
        .unwrap();
    let typed_bytes = desert_rust::serialize_to_byte_vec(typed).unwrap();
    let mut with_slash: UnboundCompiledRoute = desert_rust::deserialize(&typed_bytes).unwrap();
    if let golem_service_base::custom_api::RouteMatch::Method { trailing_slash, .. } =
        &mut with_slash.route_match
    {
        *trailing_slash = true;
    }
    let mut distinct: Vec<UnboundCompiledRoute> = desert_rust::deserialize(&encoded).unwrap();
    distinct.push(with_slash);
    validate_final_http_api_router(&typed.domain, &distinct, &HashMap::new(), &mut errors);
    assert!(errors.is_empty(), "{errors:?}");
    for (path, method, accepted) in [
        (
            vec![PathSegment::Literal {
                value: "openapi.json".into(),
            }],
            HttpMethod::Get(Empty {}),
            false,
        ),
        (
            vec![PathSegment::Literal {
                value: "openapi.json".into(),
            }],
            HttpMethod::Post(Empty {}),
            true,
        ),
        (
            vec![PathSegment::Variable {
                display_name: "other".into(),
            }],
            HttpMethod::Get(Empty {}),
            true,
        ),
    ] {
        let mut routes: Vec<UnboundCompiledRoute> = desert_rust::deserialize(&encoded).unwrap();
        let typed = routes
            .iter_mut()
            .find(|route| matches!(route.behaviour, RouteBehaviour::CallAgent(_)))
            .unwrap();
        typed.path = path;
        typed.route_match = method.into();
        let mut errors = vec![];
        validate_final_http_api_router(
            &Domain("example.com".into()),
            &routes,
            &HashMap::new(),
            &mut errors,
        );
        assert_eq!(errors.is_empty(), accepted, "{errors:?}");
    }
    for (last, accepted) in [
        (
            PathSegment::Literal {
                value: "specific".into(),
            },
            false,
        ),
        (
            PathSegment::CatchAll {
                display_name: "rest".into(),
            },
            true,
        ),
    ] {
        let mut routes: Vec<UnboundCompiledRoute> = desert_rust::deserialize(&encoded).unwrap();
        let typed = routes
            .iter_mut()
            .find(|route| matches!(route.behaviour, RouteBehaviour::CallAgent(_)))
            .unwrap();
        typed.path = vec![
            PathSegment::Literal {
                value: "hooks".into(),
            },
            last,
        ];
        typed.route_match = HttpMethod::Post(Empty {}).into();
        routes.push(UnboundCompiledRoute {
            domain: Domain("example.com".into()),
            route_id: 99,
            route_match: HttpMethod::Post(Empty {}).into(),
            path: vec![
                PathSegment::Literal {
                    value: "hooks".into(),
                },
                PathSegment::Variable {
                    display_name: "promise-id".into(),
                },
            ],
            body: golem_service_base::custom_api::RequestBodySchema::Unused,
            behaviour: RouteBehaviour::WebhookCallback(
                golem_service_base::custom_api::WebhookCallbackBehaviour {
                    component_id: ComponentId::new(),
                },
            ),
            security: crate::model::api_definition::UnboundRouteSecurity::None,
            cors: golem_service_base::custom_api::CorsOptions {
                allowed_patterns: vec![],
            },
        });
        let mut errors = vec![];
        validate_final_http_api_router(
            &Domain("example.com".into()),
            &routes,
            &HashMap::new(),
            &mut errors,
        );
        assert_eq!(errors.is_empty(), accepted, "{errors:?}");
    }
    for (callback, post, accepted) in [
        ("/auth/%63allback", false, false),
        ("/auth/%63allback/", false, true),
        ("/auth/callback", true, true),
    ] {
        let name = SecuritySchemeName("login".into());
        let scheme = SecuritySchemeDetails {
            id: golem_common::model::security_scheme::SecuritySchemeId::new(),
            name: name.clone(),
            provider_type: golem_common::model::security_scheme::Provider::Google(Empty {}),
            client_id: openidconnect::ClientId::new("test-client".into()),
            client_secret: openidconnect::ClientSecret::new("test-secret".into()),
            redirect_url: openidconnect::RedirectUrl::new(format!("https://example.com{callback}"))
                .unwrap(),
            scopes: vec![],
        };
        let mut routes: Vec<UnboundCompiledRoute> = desert_rust::deserialize(&encoded).unwrap();
        let typed = routes
            .iter_mut()
            .find(|route| matches!(route.behaviour, RouteBehaviour::CallAgent(_)))
            .unwrap();
        typed.path = vec![
            PathSegment::Literal {
                value: "auth".into(),
            },
            PathSegment::Literal {
                value: "callback".into(),
            },
        ];
        typed.route_match = if post {
            HttpMethod::Post(Empty {})
        } else {
            HttpMethod::Get(Empty {})
        }
        .into();
        typed.security = crate::model::api_definition::UnboundRouteSecurity::SecurityScheme(
            crate::model::api_definition::UnboundSecuritySchemeRouteSecurity {
                security_scheme: name.clone(),
            },
        );
        let mut errors = vec![];
        validate_final_http_api_router(
            &Domain("example.com".into()),
            &routes,
            &HashMap::from([(name, scheme)]),
            &mut errors,
        );
        assert_eq!(errors.is_empty(), accepted, "{callback}: {errors:?}");
    }
}

#[test]
fn http_mounts_compile_only_when_selected_for_deployment() {
    use golem_common::model::agent::{CorsOptions, FileMapping, HttpMountDetails};
    use golem_common::model::http_api_deployment::{
        HttpApiDeploymentId, HttpApiDeploymentRevision,
    };
    use golem_common::schema::AgentTypeKind;

    for (kind, static_files, live_files) in [
        (AgentTypeKind::HttpRouter, false, false),
        (AgentTypeKind::HttpRouter, true, false),
        (AgentTypeKind::Regular, false, true),
        (AgentTypeKind::Regular, false, false),
    ] {
        let environment = test_environment();
        let mut agent = agent_type_with_secret_config(
            AgentTypeName("site".into()),
            vec!["key".into()],
            SchemaType::string(),
        );
        agent.kind = kind;
        if kind == AgentTypeKind::HttpRouter {
            agent.mode = AgentMode::Ephemeral;
        }
        let mapping = FileMapping::compile("/", "/index.html").unwrap();
        agent.http_mount = Some(HttpMountDetails {
            path_prefix: vec![],
            auth_details: None,
            phantom_agent: false,
            cors_options: CorsOptions {
                allowed_patterns: vec![],
            },
            webhook_suffix: vec![],
            static_bindings: if static_files {
                vec![mapping.clone()]
            } else {
                vec![]
            },
            filesystem_bindings: if live_files { vec![mapping] } else { vec![] },
            openapi_provider_method: None,
        });
        agent.validate().unwrap();
        let name = agent.type_name.clone();
        let mut context = DeploymentContext {
            environment: environment.clone(),
            components: BTreeMap::new(),
            http_api_deployments: BTreeMap::new(),
            mcp_deployments: BTreeMap::new(),
            registered_agent_types: HashMap::from([(
                name.clone(),
                InProgressDeployedRegisteredAgentType {
                    agent_type: agent,
                    implemented_by: test_implementer(),
                    webhook_domain_and_segments: None,
                },
            )]),
        };
        let mut errors = vec![];
        let mut warnings = vec![];
        assert!(
            context
                .compile_http_api_routes(&HashMap::new(), &mut errors, &mut warnings)
                .is_empty()
        );
        assert!(errors.is_empty());
        let domain = Domain("example.com".into());
        context.http_api_deployments.insert(
            domain.clone(),
            HttpApiDeployment {
                scheme: Default::default(),
                id: HttpApiDeploymentId::new(),
                revision: HttpApiDeploymentRevision::INITIAL,
                environment_id: environment.id,
                domain,
                hash: diff::Hash::empty(),
                agents: BTreeMap::from([(name, Default::default())]),
                webhooks_prefix: "/webhooks".into(),
                openapi_endpoint_prefix: "/".into(),
                created_at: chrono::Utc::now(),
            },
        );
        let routes = context.compile_http_api_routes(&HashMap::new(), &mut errors, &mut warnings);
        assert!(errors.is_empty(), "{errors:?}");
        if kind == AgentTypeKind::HttpRouter || live_files {
            assert_eq!(routes.len(), 3);
            assert!(matches!(
                routes[0].route_match,
                golem_service_base::custom_api::RouteMatch::MountPrefix
            ));
            match &routes[0].behaviour {
                golem_service_base::custom_api::RouteBehaviour::HttpRouter(router) => {
                    assert!(router.handler.is_none());
                    assert_eq!(router.static_bindings.len(), usize::from(static_files));
                }
                golem_service_base::custom_api::RouteBehaviour::AgentFilesystem(filesystem) => {
                    assert_eq!(filesystem.filesystem_bindings.len(), 1)
                }
                _ => panic!("expected fallback descriptor"),
            }
        } else {
            assert_eq!(routes.len(), 2);
        }
    }
}

fn agent_type_with_secret_config(
    agent_type_name: AgentTypeName,
    path: Vec<String>,
    value_type: SchemaType,
) -> AgentTypeSchema {
    AgentTypeSchema {
        type_name: agent_type_name,
        kind: golem_common::schema::AgentTypeKind::Regular,
        description: String::new(),
        source_language: String::new(),
        schema: SchemaGraph::empty(),
        constructor: AgentConstructorSchema {
            name: None,
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::parameters([]),
        },
        methods: Vec::new(),
        dependencies: Vec::new(),
        mode: AgentMode::Durable,
        http_mount: None,
        snapshotting: Snapshotting::Disabled(Empty {}),
        config: vec![AgentConfigDeclarationSchema {
            source: AgentConfigSource::Secret,
            path,
            value_type,
        }],
    }
}

fn stored_agent_secret(
    path: &[String],
    secret_type: SchemaGraph,
    secret_value: Option<SchemaValue>,
) -> AgentSecret {
    AgentSecret {
        id: AgentSecretId::new(),
        environment_id: EnvironmentId::new(),
        path: CanonicalAgentSecretPath::from_path_in_unknown_casing(path),
        revision: AgentSecretRevision::INITIAL,
        secret_type,
        secret_value,
    }
}

fn test_tool(name: &str) -> Tool {
    Tool {
        version: "1.0.0".to_string(),
        commands: CommandTree {
            nodes: vec![CommandNode {
                name: name.to_string(),
                aliases: Vec::new(),
                doc: Doc::default(),
                globals: Globals::default(),
                subcommands: Vec::new(),
                body: None,
            }],
        },
        schema: SchemaGraph::empty(),
    }
}

fn executable_test_tool(root: &str, command: &str) -> Tool {
    let node = |name: &str, subcommands, body| CommandNode {
        name: name.to_string(),
        aliases: Vec::new(),
        doc: Doc::default(),
        globals: Globals::default(),
        subcommands,
        body,
    };
    let body = CommandBody {
        positionals: Positionals::default(),
        options: Vec::new(),
        flags: Vec::new(),
        constraints: Vec::new(),
        stdin: None,
        stdout: None,
        result: None,
        errors: Vec::new(),
        annotations: None,
    };
    Tool {
        version: "1.0.0".to_string(),
        commands: CommandTree {
            nodes: vec![
                node(root, vec![CommandIndex(1)], None),
                node(command, Vec::new(), Some(body)),
            ],
        },
        schema: SchemaGraph::empty(),
    }
}

fn native_tool_component(name: &str, tool_name: &str, definition: Tool) -> Component {
    test_tool_component(
        name,
        BTreeMap::from([(
            ToolName::try_from(tool_name).unwrap(),
            ToolDeploymentMetadata {
                definition,
                provision: ToolProvisionConfig::default(),
                environment_binding: None,
                component_bindings: BTreeMap::from([(
                    ComponentName(name.to_string()),
                    ToolBindingInput::default(),
                )]),
                agent_bindings: BTreeMap::new(),
            },
        )]),
    )
}

fn mcp_deployment(
    environment_id: EnvironmentId,
    domain: &str,
    agents: BTreeMap<AgentTypeName, McpDeploymentAgentOptions>,
    tools: BTreeMap<ToolName, McpDeploymentToolOptions>,
) -> McpDeployment {
    McpDeployment {
        id: McpDeploymentId::new(),
        revision: McpDeploymentRevision::INITIAL,
        environment_id,
        domain: Domain(domain.to_string()),
        hash: diff::Hash::empty(),
        agents,
        tools,
        created_at: chrono::Utc::now(),
    }
}

fn test_tool_component(name: &str, tools: BTreeMap<ToolName, ToolDeploymentMetadata>) -> Component {
    Component {
        id: ComponentId::new(),
        revision: ComponentRevision::INITIAL,
        environment_id: EnvironmentId::new(),
        component_name: ComponentName(name.to_string()),
        hash: diff::Hash::empty(),
        application_id: ApplicationId::new(),
        account_id: AccountId::new(),
        account_email: AccountEmail::new("owner@example.com"),
        application_name: ApplicationName::try_from("app").unwrap(),
        environment_name: EnvironmentName::try_from("dev").unwrap(),
        component_size: 0,
        metadata: ComponentMetadata::from_parts_with_tools(
            KnownExports {
                tool_guest_interface: Some("golem:tool/guest@0.1.0".to_string()),
                ..KnownExports::default()
            },
            Vec::new(),
            None,
            None,
            Vec::new(),
            BTreeMap::new(),
            tools,
        ),
        created_at: chrono::Utc::now(),
        wasm_hash: diff::Hash::empty(),
        object_store_key: String::new(),
    }
}

fn test_remote_tool(
    name: &str,
    environment_binding: Option<ToolBindingInput>,
    agent_bindings: BTreeMap<AgentTypeName, ToolBindingInput>,
) -> (RemoteToolDeployment, Option<ResolvedGrantedToolRelease>) {
    let name = ToolName::try_from(name).unwrap();
    let owner_account_id = AccountId::new();
    let owner_email = AccountEmail::new("publisher@example.com");
    let release_id = ToolReleaseId::new();
    let definition = test_tool(name.as_str());
    let release = ToolRelease {
        id: release_id,
        owner_account_id,
        name: name.clone(),
        version: definition.version.clone(),
        source: ToolSource::Component {
            component_id: ComponentId::new(),
            component_revision: ComponentRevision::INITIAL,
            component_name: ComponentName("publisher-tools".to_string()),
        },
        definition: definition.clone(),
        metadata_version: TOOL_METADATA_WIT_VERSION.to_string(),
        metadata_digest: golem_common::model::tool_release::tool_metadata_digest(
            TOOL_METADATA_WIT_VERSION,
            &definition,
        )
        .unwrap(),
        immutable: true,
        lifecycle: ToolReleaseLifecycle::Published,
        origin: ToolReleaseOrigin::Ordinary,
        system_availability: None,
        created_at: chrono::Utc::now(),
        created_by: owner_account_id,
        state_changed_at: chrono::Utc::now(),
        state_changed_by: owner_account_id,
    };
    (
        RemoteToolDeployment {
            name,
            release: ToolReleaseReference::ById(ToolReleaseById { release_id }),
            provision: ToolProvisionConfig {
                config: NormalizedJsonValue::new(json!({ "consumer": true })),
                ..ToolProvisionConfig::default()
            },
            environment_binding,
            component_bindings: BTreeMap::new(),
            agent_bindings,
        },
        Some(ResolvedGrantedToolRelease {
            release,
            owner: AccountSummary {
                id: owner_account_id,
                name: "Publisher".to_string(),
                email: owner_email,
            },
        }),
    )
}

fn invalid_component_bindings() -> BTreeMap<ComponentName, ToolBindingInput> {
    BTreeMap::from([
        (
            ComponentName("wrong-version".to_string()),
            ToolBindingInput {
                version: Some("2.0.0".to_string()),
                ..ToolBindingInput::default()
            },
        ),
        (
            ComponentName("wrong-account".to_string()),
            ToolBindingInput {
                account: Some(AccountEmail::new("other@example.com")),
                ..ToolBindingInput::default()
            },
        ),
        (
            ComponentName("nonobject-parameters".to_string()),
            ToolBindingInput {
                parameters: NormalizedJsonValue::new(json!(["not", "an", "object"])),
                ..ToolBindingInput::default()
            },
        ),
    ])
}

fn assert_component_binding_errors(errors: &[DeployValidationError]) {
    assert_eq!(errors.len(), 3);
    assert!(errors.iter().any(|error| matches!(
        error,
        DeployValidationError::ToolBindingVersionMismatch {
            agent_type: None,
            ..
        }
    )));
    assert!(errors.iter().any(|error| matches!(
        error,
        DeployValidationError::ToolBindingAccountMismatch {
            agent_type: None,
            ..
        }
    )));
    assert!(errors.iter().any(|error| matches!(
        error,
        DeployValidationError::ToolBindingParametersMustBeObject {
            agent_type: None,
            ..
        }
    )));
}

fn test_registered_agent_type(
    agent_type_name: &str,
) -> (AgentTypeName, InProgressDeployedRegisteredAgentType) {
    let agent_type_name = AgentTypeName(agent_type_name.to_string());
    let agent_type = agent_type_with_secret_config(
        agent_type_name.clone(),
        vec!["apiKey".to_string()],
        SchemaType::secret(SecretSpec {
            inner: Box::new(SchemaType::string()),
            category: None,
        }),
    );
    (
        agent_type_name,
        InProgressDeployedRegisteredAgentType {
            agent_type,
            implemented_by: test_implementer(),
            webhook_domain_and_segments: None,
        },
    )
}

fn compile_test_mcp() -> (CompiledMcp, Vec<RegisteredAgentTypeSchema>) {
    let environment = test_environment();
    let domain = Domain("mcp.example.com".to_string());
    let (agent_a_name, agent_a) = test_registered_agent_type("AgentA");
    let (agent_b_name, agent_b) = test_registered_agent_type("AgentB");
    let (agent_c_name, agent_c) = test_registered_agent_type("AgentC");
    let expected = vec![
        RegisteredAgentTypeSchema {
            agent_type: agent_a.agent_type.clone(),
            implemented_by: agent_a.implemented_by.clone(),
        },
        RegisteredAgentTypeSchema {
            agent_type: agent_b.agent_type.clone(),
            implemented_by: agent_b.implemented_by.clone(),
        },
    ];
    let mcp_deployment = McpDeployment {
        id: McpDeploymentId::new(),
        revision: McpDeploymentRevision::INITIAL,
        environment_id: environment.id,
        domain: domain.clone(),
        hash: diff::Hash::empty(),
        agents: BTreeMap::from([
            (agent_b_name.clone(), McpDeploymentAgentOptions::default()),
            (agent_a_name.clone(), McpDeploymentAgentOptions::default()),
        ]),
        tools: BTreeMap::new(),
        created_at: chrono::Utc::now(),
    };
    let context = DeploymentContext {
        environment,
        components: BTreeMap::new(),
        http_api_deployments: BTreeMap::new(),
        mcp_deployments: BTreeMap::from([(domain, mcp_deployment)]),
        registered_agent_types: HashMap::from([
            (agent_a_name, agent_a),
            (agent_b_name, agent_b),
            (agent_c_name, agent_c),
        ]),
    };
    let mut errors = Vec::new();
    let mut compiled = context.compile_mcp_deployments(
        AccountId::new(),
        golem_common::model::deployment::DeploymentRevision::INITIAL,
        &HashMap::new(),
        &CompiledTools {
            registered_tools: Vec::new(),
            agent_tool_bindings: Vec::new(),
        },
        &[],
        &mut errors,
    );

    assert!(errors.is_empty());
    assert_eq!(compiled.len(), 1);

    (compiled.pop().unwrap(), expected)
}

#[test]
fn compile_mcp_deployments_includes_selected_registered_agent_types() {
    let (compiled, expected) = compile_test_mcp();

    assert_eq!(compiled.registered_agent_types, expected);
}

#[test]
fn compiled_mcp_blob_round_trip_preserves_registered_agent_types() {
    let (compiled, expected) = compile_test_mcp();
    let record = DeploymentCompiledMcpRecord::from_model(compiled);
    let serialized = record.mcp_data.serialize().unwrap().clone();
    let mcp_data: Blob<CompiledMcpData> = Blob::deserialze(serialized).unwrap();
    let restored = CompiledMcp::try_from(DeploymentCompiledMcpRecord {
        account_id: record.account_id,
        account_email: record.account_email,
        environment_id: record.environment_id,
        deployment_revision_id: record.deployment_revision_id,
        domain: record.domain,
        mcp_data,
    })
    .unwrap();

    assert_eq!(restored.registered_agent_types, expected);
}

#[test]
fn compile_mcp_mixes_agents_and_native_tools_and_persists_pinned_definition() {
    let environment = test_environment();
    let (agent_name, agent) = test_registered_agent_type("AgentA");
    let definition = executable_test_tool("foo", "bar");
    let component = native_tool_component("owner", "foo", definition.clone());
    let deployment = mcp_deployment(
        environment.id,
        "mixed.example.com",
        BTreeMap::from([(agent_name.clone(), McpDeploymentAgentOptions::default())]),
        BTreeMap::from([(
            ToolName::try_from("foo").unwrap(),
            McpDeploymentToolOptions {
                owner_component: component.component_name.clone(),
                security_scheme: None,
                include: None,
                exclude: None,
            },
        )]),
    );
    let context = DeploymentContext {
        environment,
        components: BTreeMap::from([(component.component_name.clone(), component.clone())]),
        http_api_deployments: BTreeMap::new(),
        mcp_deployments: BTreeMap::from([(deployment.domain.clone(), deployment)]),
        registered_agent_types: HashMap::from([(agent_name, agent)]),
    };
    let mut errors = Vec::new();
    let tools = context.compile_tools(
        golem_common::model::deployment::DeploymentRevision::INITIAL,
        &mut errors,
        &mut Vec::new(),
    );
    let compiled = context.compile_mcp_deployments(
        AccountId::new(),
        golem_common::model::deployment::DeploymentRevision::INITIAL,
        &HashMap::new(),
        &tools,
        &[],
        &mut errors,
    );

    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(compiled.len(), 1);
    assert_eq!(compiled[0].registered_agent_types.len(), 1);
    assert_eq!(compiled[0].tools.len(), 1);
    assert_eq!(compiled[0].tools[0].mcp_name, "foo_bar");
    assert_eq!(compiled[0].tools[0].owner_component_id, component.id);
    assert_eq!(compiled[0].tools[0].definition, definition);

    let record = DeploymentCompiledMcpRecord::from_model(compiled.into_iter().next().unwrap());
    let bytes = record.mcp_data.serialize().unwrap().clone();
    let restored = CompiledMcp::try_from(DeploymentCompiledMcpRecord {
        mcp_data: Blob::deserialze(bytes).unwrap(),
        ..record
    })
    .unwrap();
    assert_eq!(restored.tools.len(), 1);
    assert_eq!(restored.tools[0].owner_component_id, component.id);
    assert_eq!(restored.tools[0].definition, definition);

    let presented = executable_test_tool("foo", "wrapped");
    let chain = golem_common::model::tool_middleware::CompiledToolMiddlewareChain {
        deployment_revision: golem_common::model::deployment::DeploymentRevision::INITIAL,
        owner: ToolBindingOwner::ComponentBaseline {
            component_id: component.id,
        },
        tool_name: ToolName::try_from("foo").unwrap(),
        effective_definition: presented.clone(),
        occurrences: Vec::new(),
    };
    let other_owner = golem_common::model::tool_middleware::CompiledToolMiddlewareChain {
        owner: ToolBindingOwner::ComponentBaseline {
            component_id: ComponentId::new(),
        },
        effective_definition: executable_test_tool("foo", "wrong-owner"),
        ..chain.clone()
    };
    let compiled = context.compile_mcp_deployments(
        AccountId::new(),
        golem_common::model::deployment::DeploymentRevision::INITIAL,
        &HashMap::new(),
        &tools,
        &[other_owner, chain],
        &mut errors,
    );
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(compiled[0].tools[0].mcp_name, "foo_wrapped");
    assert_eq!(compiled[0].tools[0].definition, presented);
}

#[test]
fn compile_mcp_rejects_empty_invalid_owner_binding_and_include_exclude() {
    let environment = test_environment();
    let definition = executable_test_tool("foo", "bar");
    let unbound = test_tool_component(
        "unbound",
        BTreeMap::from([(
            ToolName::try_from("foo").unwrap(),
            ToolDeploymentMetadata {
                definition,
                provision: ToolProvisionConfig::default(),
                environment_binding: None,
                component_bindings: BTreeMap::new(),
                agent_bindings: BTreeMap::new(),
            },
        )]),
    );
    let options = |owner: &str, both: bool| McpDeploymentToolOptions {
        owner_component: ComponentName(owner.to_string()),
        security_scheme: None,
        include: both.then(|| vec!["bar".to_string()]),
        exclude: both.then(|| vec!["bar".to_string()]),
    };
    let deployments = [
        mcp_deployment(
            environment.id,
            "empty.example.com",
            BTreeMap::new(),
            BTreeMap::new(),
        ),
        mcp_deployment(
            environment.id,
            "owner.example.com",
            BTreeMap::new(),
            BTreeMap::from([(
                ToolName::try_from("foo").unwrap(),
                options("missing", false),
            )]),
        ),
        mcp_deployment(
            environment.id,
            "binding.example.com",
            BTreeMap::new(),
            BTreeMap::from([(
                ToolName::try_from("foo").unwrap(),
                options("unbound", false),
            )]),
        ),
        mcp_deployment(
            environment.id,
            "filters.example.com",
            BTreeMap::new(),
            BTreeMap::from([(ToolName::try_from("foo").unwrap(), options("unbound", true))]),
        ),
    ];
    let context = DeploymentContext {
        environment,
        components: BTreeMap::from([(unbound.component_name.clone(), unbound)]),
        http_api_deployments: BTreeMap::new(),
        mcp_deployments: deployments
            .into_iter()
            .map(|d| (d.domain.clone(), d))
            .collect(),
        registered_agent_types: HashMap::new(),
    };
    let mut errors = Vec::new();
    let tools = context.compile_tools(
        golem_common::model::deployment::DeploymentRevision::INITIAL,
        &mut errors,
        &mut Vec::new(),
    );
    context.compile_mcp_deployments(
        AccountId::new(),
        golem_common::model::deployment::DeploymentRevision::INITIAL,
        &HashMap::new(),
        &tools,
        &[],
        &mut errors,
    );

    assert!(
        errors
            .iter()
            .any(|e| matches!(e, DeployValidationError::McpDeploymentEmpty { .. }))
    );
    for message in [
        "owner component",
        "effective component-baseline binding",
        "mutually exclusive",
    ] {
        assert!(errors.iter().any(|e| matches!(e, DeployValidationError::McpDeploymentInvalidTool { error, .. } if error.contains(message))), "missing {message}: {errors:?}");
    }
}

#[test]
fn compile_mcp_rejects_cross_tool_normalized_name_collision_and_auth_conflict() {
    let environment = test_environment();
    let first = native_tool_component("first", "foo-bar", executable_test_tool("foo-bar", "baz"));
    let second = native_tool_component("second", "foo", executable_test_tool("foo", "bar-baz"));
    let scheme_a = SecuritySchemeName("scheme-a".to_string());
    let scheme_b = SecuritySchemeName("scheme-b".to_string());
    let tool_options = |component: &Component, security_scheme| McpDeploymentToolOptions {
        owner_component: component.component_name.clone(),
        security_scheme,
        include: None,
        exclude: None,
    };
    let deployment = mcp_deployment(
        environment.id,
        "collision.example.com",
        BTreeMap::new(),
        BTreeMap::from([
            (
                ToolName::try_from("foo-bar").unwrap(),
                tool_options(&first, Some(scheme_a)),
            ),
            (
                ToolName::try_from("foo").unwrap(),
                tool_options(&second, Some(scheme_b)),
            ),
        ]),
    );
    let context = DeploymentContext {
        environment,
        components: BTreeMap::from([
            (first.component_name.clone(), first),
            (second.component_name.clone(), second),
        ]),
        http_api_deployments: BTreeMap::new(),
        mcp_deployments: BTreeMap::from([(deployment.domain.clone(), deployment)]),
        registered_agent_types: HashMap::new(),
    };
    let mut errors = Vec::new();
    let tools = context.compile_tools(
        golem_common::model::deployment::DeploymentRevision::INITIAL,
        &mut errors,
        &mut Vec::new(),
    );
    context.compile_mcp_deployments(
        AccountId::new(),
        golem_common::model::deployment::DeploymentRevision::INITIAL,
        &HashMap::new(),
        &tools,
        &[],
        &mut errors,
    );

    assert!(errors.iter().any(|e| matches!(e, DeployValidationError::McpDeploymentToolNameCollision { name, .. } if name == "foo_bar_baz")), "{errors:?}");
    assert!(
        errors.iter().any(|e| matches!(
            e,
            DeployValidationError::McpDeploymentConflictingSecuritySchemes { .. }
        )),
        "{errors:?}"
    );
}

#[test]
fn compile_tools_registers_unbound_tool_without_agent_bindings() {
    let tool_name = ToolName::try_from("grep").unwrap();
    let component_bindings = BTreeMap::from([(
        ComponentName("consumer".to_string()),
        ToolBindingInput {
            parameters: NormalizedJsonValue::new(json!({ "scope": "component" })),
            ..ToolBindingInput::default()
        },
    )]);
    let component = test_tool_component(
        "tools",
        BTreeMap::from([(
            tool_name.clone(),
            ToolDeploymentMetadata {
                definition: test_tool(tool_name.as_str()),
                provision: ToolProvisionConfig::default(),
                environment_binding: None,
                component_bindings: component_bindings.clone(),
                agent_bindings: BTreeMap::new(),
            },
        )]),
    );
    let context = DeploymentContext {
        environment: test_environment(),
        components: BTreeMap::from([(component.component_name.clone(), component)]),
        http_api_deployments: BTreeMap::new(),
        mcp_deployments: BTreeMap::new(),
        registered_agent_types: HashMap::new(),
    };
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    let compiled = context.compile_tools(
        golem_common::model::deployment::DeploymentRevision::INITIAL,
        &mut errors,
        &mut warnings,
    );

    assert!(errors.is_empty());
    assert!(warnings.is_empty());
    assert_eq!(compiled.registered_tools.len(), 1);
    assert_eq!(compiled.registered_tools[0].definition.name(), Some("grep"));
    assert_eq!(
        compiled.registered_tools[0].component_bindings,
        component_bindings
    );
    assert!(compiled.agent_tool_bindings.is_empty());
}

#[test]
fn compile_tools_validates_local_component_bindings_without_agent_bindings() {
    let tool_name = ToolName::try_from("grep").unwrap();
    let component = test_tool_component(
        "tools",
        BTreeMap::from([(
            tool_name.clone(),
            ToolDeploymentMetadata {
                definition: test_tool(tool_name.as_str()),
                provision: ToolProvisionConfig::default(),
                environment_binding: None,
                component_bindings: invalid_component_bindings(),
                agent_bindings: BTreeMap::new(),
            },
        )]),
    );
    let context = DeploymentContext {
        environment: test_environment(),
        components: BTreeMap::from([(component.component_name.clone(), component)]),
        http_api_deployments: BTreeMap::new(),
        mcp_deployments: BTreeMap::new(),
        registered_agent_types: HashMap::new(),
    };
    let mut errors = Vec::new();

    let compiled = context.compile_tools(
        golem_common::model::deployment::DeploymentRevision::INITIAL,
        &mut errors,
        &mut Vec::new(),
    );

    assert_component_binding_errors(&errors);
    assert_eq!(compiled.registered_tools.len(), 1);
    assert!(compiled.registered_tools[0].component_bindings.is_empty());
    assert!(compiled.agent_tool_bindings.is_empty());
}

#[test]
fn compile_tools_registers_remote_source_with_consumer_provision_and_bindings() {
    let (agent_a_name, agent_a) = test_registered_agent_type("AgentA");
    let (agent_b_name, agent_b) = test_registered_agent_type("AgentB");
    let mut remote = test_remote_tool(
        "grep",
        Some(ToolBindingInput {
            parameters: NormalizedJsonValue::new(json!({ "scope": "environment" })),
            ..ToolBindingInput::default()
        }),
        BTreeMap::from([(
            agent_a_name.clone(),
            ToolBindingInput {
                parameters: NormalizedJsonValue::new(json!({ "scope": "agent" })),
                ..ToolBindingInput::default()
            },
        )]),
    );
    let baseline_component = test_tool_component("baseline", BTreeMap::new());
    remote.0.component_bindings.insert(
        baseline_component.component_name.clone(),
        ToolBindingInput {
            parameters: NormalizedJsonValue::new(json!({ "scope": "component" })),
            ..ToolBindingInput::default()
        },
    );
    let context = DeploymentContext {
        environment: test_environment(),
        components: BTreeMap::from([(
            baseline_component.component_name.clone(),
            baseline_component.clone(),
        )]),
        http_api_deployments: BTreeMap::new(),
        mcp_deployments: BTreeMap::new(),
        registered_agent_types: HashMap::from([
            (agent_a_name.clone(), agent_a),
            (agent_b_name.clone(), agent_b),
        ]),
    };
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    let compiled = context.compile_tools_with_remote(
        golem_common::model::deployment::DeploymentRevision::INITIAL,
        std::slice::from_ref(&remote),
        &mut errors,
        &mut warnings,
    );

    assert!(errors.is_empty());
    assert!(warnings.is_empty());
    assert_eq!(compiled.registered_tools.len(), 1);
    let registered = &compiled.registered_tools[0];
    assert_eq!(
        registered.release_id,
        Some(remote.1.as_ref().unwrap().release.id)
    );
    assert_eq!(registered.source, remote.1.as_ref().unwrap().release.source);
    assert_eq!(registered.provision, remote.0.provision);
    assert_eq!(
        registered.owner_account_email.as_str(),
        "publisher@example.com"
    );
    assert_eq!(compiled.agent_tool_bindings.len(), 3);
    let bindings = compiled
        .agent_tool_bindings
        .iter()
        .filter_map(|binding| match &binding.owner {
            ToolBindingOwner::AgentType { agent_type_name } => {
                Some((agent_type_name.clone(), binding.parameters.0.clone()))
            }
            ToolBindingOwner::ComponentBaseline { .. } => None,
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(bindings[&agent_a_name], json!({ "scope": "agent" }));
    assert_eq!(bindings[&agent_b_name], json!({ "scope": "environment" }));
    let baseline = compiled
        .agent_tool_bindings
        .iter()
        .find(|binding| {
            binding.owner
                == ToolBindingOwner::ComponentBaseline {
                    component_id: baseline_component.id,
                }
        })
        .unwrap();
    assert_eq!(baseline.parameters.0, json!({ "scope": "component" }));

    let unbound = test_remote_tool("git", None, BTreeMap::new());
    let compiled = context.compile_tools_with_remote(
        golem_common::model::deployment::DeploymentRevision::INITIAL,
        &[unbound],
        &mut Vec::new(),
        &mut Vec::new(),
    );
    assert_eq!(compiled.registered_tools.len(), 1);
    assert!(compiled.agent_tool_bindings.is_empty());
}

#[test]
fn compile_tools_validates_remote_component_bindings_without_agent_bindings() {
    let mut remote = test_remote_tool("grep", None, BTreeMap::new());
    remote.0.component_bindings = invalid_component_bindings();
    let context = DeploymentContext {
        environment: test_environment(),
        components: BTreeMap::new(),
        http_api_deployments: BTreeMap::new(),
        mcp_deployments: BTreeMap::new(),
        registered_agent_types: HashMap::new(),
    };
    let mut errors = Vec::new();

    let compiled = context.compile_tools_with_remote(
        golem_common::model::deployment::DeploymentRevision::INITIAL,
        &[remote],
        &mut errors,
        &mut Vec::new(),
    );

    assert_component_binding_errors(&errors);
    assert_eq!(compiled.registered_tools.len(), 1);
    assert!(compiled.registered_tools[0].component_bindings.is_empty());
    assert!(compiled.agent_tool_bindings.is_empty());
}

#[test]
fn zero_agent_remote_component_binding_hash_uses_effective_binding_and_matches_cli() {
    let component = test_tool_component("consumer", BTreeMap::new());
    let context = DeploymentContext {
        environment: test_environment(),
        components: BTreeMap::from([(component.component_name.clone(), component.clone())]),
        http_api_deployments: BTreeMap::new(),
        mcp_deployments: BTreeMap::new(),
        registered_agent_types: HashMap::new(),
    };
    let hash = |parameters, readable| {
        let environment_binding = ToolBindingInput {
            parameters: NormalizedJsonValue::new(parameters),
            secret_keys_readable: readable,
            ..ToolBindingInput::default()
        };
        let component_binding = ToolBindingInput {
            parameters: NormalizedJsonValue::new(json!({ "component": true })),
            ..ToolBindingInput::default()
        };
        let mut remote =
            test_remote_tool("grep", Some(environment_binding.clone()), BTreeMap::new());
        remote
            .0
            .component_bindings
            .insert(component.component_name.clone(), component_binding.clone());
        let mut errors = Vec::new();
        let compiled = context.compile_tools_with_remote(
            golem_common::model::deployment::DeploymentRevision::INITIAL,
            std::slice::from_ref(&remote),
            &mut errors,
            &mut Vec::new(),
        );
        assert!(errors.is_empty());
        assert!(context.registered_agent_types.is_empty());

        let server_hash = context
            .hash_with_tools(
                &compiled,
                &[],
                &[],
                &[],
                &[],
                Default::default(),
                &BTreeMap::new(),
                &BTreeMap::new(),
            )
            .unwrap();
        let release = &remote.1.as_ref().unwrap().release;
        let effective =
            diff::effective_tool_binding(Some(&environment_binding), Some(&component_binding))
                .unwrap()
                .0;
        let cli_hash = diff::Deployment {
            components: BTreeMap::from([(
                component.component_name.0.clone(),
                HashOf::from_hash(component.hash),
            )]),
            remote_tools: BTreeMap::from([(
                "grep".to_string(),
                diff::RemoteToolDeployment {
                    release_id: release.id,
                    version: release.version.clone(),
                    source_digest: golem_common::model::tool_release::tool_source_digest(
                        &release.source,
                    ),
                    owner_account_id: release.owner_account_id,
                    owner_account_email: remote.1.as_ref().unwrap().owner.email.clone(),
                    metadata_version: release.metadata_version.clone(),
                    metadata_digest: release.metadata_digest,
                    provision: remote.0.provision.clone(),
                    component_bindings: BTreeMap::from([(
                        component.component_name.0.clone(),
                        effective,
                    )]),
                    bindings: BTreeMap::new(),
                }
                .into(),
            )]),
            ..diff::Deployment::default()
        }
        .hash()
        .unwrap();
        assert_eq!(cli_hash, server_hash);
        server_hash
    };

    let baseline = hash(json!({ "environment": 1 }), SecretKeyScope::All);
    assert_ne!(
        baseline,
        hash(json!({ "environment": 2 }), SecretKeyScope::All)
    );
    assert_ne!(
        baseline,
        hash(
            json!({ "environment": 1 }),
            SecretKeyScope::Keys(BTreeSet::new())
        )
    );
}

#[test]
fn compile_tools_accumulates_remote_collisions_and_unavailable_references() {
    let grep = ToolName::try_from("grep").unwrap();
    let local = test_tool_component(
        "local-tools",
        BTreeMap::from([(
            grep.clone(),
            ToolDeploymentMetadata {
                definition: test_tool(grep.as_str()),
                provision: ToolProvisionConfig::default(),
                environment_binding: None,
                component_bindings: BTreeMap::new(),
                agent_bindings: BTreeMap::new(),
            },
        )]),
    );
    let mut unavailable_a = test_remote_tool("missing-a", None, BTreeMap::new());
    unavailable_a.1 = None;
    let mut unavailable_b = test_remote_tool("missing-b", None, BTreeMap::new());
    unavailable_b.1 = None;
    let remote_tools = vec![
        test_remote_tool("grep", None, BTreeMap::new()),
        test_remote_tool("git", None, BTreeMap::new()),
        test_remote_tool("git", None, BTreeMap::new()),
        unavailable_a,
        unavailable_b,
    ];
    let context = DeploymentContext {
        environment: test_environment(),
        components: BTreeMap::from([(local.component_name.clone(), local)]),
        http_api_deployments: BTreeMap::new(),
        mcp_deployments: BTreeMap::new(),
        registered_agent_types: HashMap::new(),
    };
    let mut errors = Vec::new();

    let compiled = context.compile_tools_with_remote(
        golem_common::model::deployment::DeploymentRevision::INITIAL,
        &remote_tools,
        &mut errors,
        &mut Vec::new(),
    );

    assert!(compiled.registered_tools.is_empty());
    assert_eq!(
        errors
            .iter()
            .filter(|error| matches!(error, DeployValidationError::ToolSourceCollision { .. }))
            .count(),
        2
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| matches!(error, DeployValidationError::RemoteToolUnavailable { .. }))
            .count(),
        2
    );
}

#[test]
fn compile_tools_inherits_environment_tools_and_adds_agent_tools() {
    let grep = ToolName::try_from("grep").unwrap();
    let git = ToolName::try_from("git").unwrap();
    let (agent_a_name, agent_a) = test_registered_agent_type("AgentA");
    let (agent_b_name, agent_b) = test_registered_agent_type("AgentB");
    let component = test_tool_component(
        "tools",
        BTreeMap::from([
            (
                grep.clone(),
                ToolDeploymentMetadata {
                    definition: test_tool(grep.as_str()),
                    provision: ToolProvisionConfig::default(),
                    environment_binding: Some(ToolBindingInput::default()),
                    component_bindings: BTreeMap::new(),
                    agent_bindings: BTreeMap::new(),
                },
            ),
            (
                git.clone(),
                ToolDeploymentMetadata {
                    definition: test_tool(git.as_str()),
                    provision: ToolProvisionConfig::default(),
                    environment_binding: None,
                    component_bindings: BTreeMap::new(),
                    agent_bindings: BTreeMap::from([(
                        agent_a_name.clone(),
                        ToolBindingInput::default(),
                    )]),
                },
            ),
        ]),
    );
    let context = DeploymentContext {
        environment: test_environment(),
        components: BTreeMap::from([(component.component_name.clone(), component)]),
        http_api_deployments: BTreeMap::new(),
        mcp_deployments: BTreeMap::new(),
        registered_agent_types: HashMap::from([
            (agent_a_name.clone(), agent_a),
            (agent_b_name.clone(), agent_b),
        ]),
    };
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    let compiled = context.compile_tools(
        golem_common::model::deployment::DeploymentRevision::INITIAL,
        &mut errors,
        &mut warnings,
    );
    let bindings = compiled
        .agent_tool_bindings
        .iter()
        .filter_map(|binding| match &binding.owner {
            ToolBindingOwner::AgentType { agent_type_name } => {
                Some((agent_type_name.clone(), binding.tool_name.clone()))
            }
            ToolBindingOwner::ComponentBaseline { .. } => None,
        })
        .collect::<BTreeSet<_>>();

    assert!(errors.is_empty());
    assert!(warnings.is_empty());
    assert_eq!(compiled.registered_tools.len(), 2);
    assert_eq!(
        bindings,
        BTreeSet::from([
            (agent_a_name.clone(), grep.clone()),
            (agent_a_name, git),
            (agent_b_name, grep),
        ])
    );
}

#[test]
fn compile_tools_rejects_explicit_prepend_on_local_environment_binding() {
    let tool_name = ToolName::try_from("grep").unwrap();
    let (agent_name, agent_type) = test_registered_agent_type("AgentA");
    let component = test_tool_component(
        "tools",
        BTreeMap::from([(
            tool_name.clone(),
            ToolDeploymentMetadata {
                definition: test_tool(tool_name.as_str()),
                provision: ToolProvisionConfig::default(),
                environment_binding: Some(ToolBindingInput {
                    middleware_merge_mode: Some(ToolMiddlewareMergeMode::Prepend),
                    ..ToolBindingInput::default()
                }),
                component_bindings: BTreeMap::new(),
                agent_bindings: BTreeMap::from([(
                    agent_name.clone(),
                    ToolBindingInput {
                        middleware_merge_mode: Some(ToolMiddlewareMergeMode::Prepend),
                        ..ToolBindingInput::default()
                    },
                )]),
            },
        )]),
    );
    let context = DeploymentContext {
        environment: test_environment(),
        components: BTreeMap::from([(component.component_name.clone(), component)]),
        http_api_deployments: BTreeMap::new(),
        mcp_deployments: BTreeMap::new(),
        registered_agent_types: HashMap::from([(agent_name.clone(), agent_type)]),
    };
    let mut errors = Vec::new();

    let compiled = context.compile_tools(
        golem_common::model::deployment::DeploymentRevision::INITIAL,
        &mut errors,
        &mut Vec::new(),
    );

    assert_eq!(
        errors,
        vec![
            DeployValidationError::ToolBindingEnvironmentMiddlewareMergeMode {
                tool_name: tool_name.clone(),
            }
        ]
    );
    assert_eq!(compiled.registered_tools.len(), 1);
    assert_eq!(compiled.agent_tool_bindings.len(), 1);
    assert_eq!(
        compiled.agent_tool_bindings[0].owner,
        ToolBindingOwner::AgentType {
            agent_type_name: agent_name
        }
    );
    assert_eq!(compiled.agent_tool_bindings[0].tool_name, tool_name);
}

#[test]
fn compile_tools_rejects_explicit_prepend_on_remote_environment_binding() {
    let tool_name = ToolName::try_from("grep").unwrap();
    let (agent_name, agent_type) = test_registered_agent_type("AgentA");
    let remote = test_remote_tool(
        tool_name.as_str(),
        Some(ToolBindingInput {
            middleware_merge_mode: Some(ToolMiddlewareMergeMode::Prepend),
            ..ToolBindingInput::default()
        }),
        BTreeMap::from([(
            agent_name.clone(),
            ToolBindingInput {
                middleware_merge_mode: Some(ToolMiddlewareMergeMode::Prepend),
                ..ToolBindingInput::default()
            },
        )]),
    );
    let context = DeploymentContext {
        environment: test_environment(),
        components: BTreeMap::new(),
        http_api_deployments: BTreeMap::new(),
        mcp_deployments: BTreeMap::new(),
        registered_agent_types: HashMap::from([(agent_name.clone(), agent_type)]),
    };
    let mut errors = Vec::new();

    let compiled = context.compile_tools_with_remote(
        golem_common::model::deployment::DeploymentRevision::INITIAL,
        &[remote],
        &mut errors,
        &mut Vec::new(),
    );

    assert_eq!(
        errors,
        vec![
            DeployValidationError::ToolBindingEnvironmentMiddlewareMergeMode {
                tool_name: tool_name.clone(),
            }
        ]
    );
    assert_eq!(compiled.registered_tools.len(), 1);
    assert_eq!(compiled.agent_tool_bindings.len(), 1);
    assert_eq!(
        compiled.agent_tool_bindings[0].owner,
        ToolBindingOwner::AgentType {
            agent_type_name: agent_name
        }
    );
    assert_eq!(compiled.agent_tool_bindings[0].tool_name, tool_name);
}

#[test]
fn compile_tools_accumulates_independent_binding_errors() {
    let tool_name = ToolName::try_from("grep").unwrap();
    let (agent_name, agent_type) = test_registered_agent_type("AgentA");
    let invalid_binding = ToolBindingInput {
        version: Some("2.0.0".to_string()),
        parameters: NormalizedJsonValue::new(json!(["not", "an", "object"])),
        account: Some(AccountEmail::new("other@example.com")),
        config_keys_readable: Default::default(),
        secret_keys_readable: SecretKeyScope::All,
        secret_keys_revealable: SecretKeyScope::All,
        ..ToolBindingInput::default()
    };
    let component = test_tool_component(
        "tools",
        BTreeMap::from([(
            tool_name.clone(),
            ToolDeploymentMetadata {
                definition: test_tool(tool_name.as_str()),
                provision: ToolProvisionConfig::default(),
                environment_binding: Some(invalid_binding.clone()),
                component_bindings: BTreeMap::new(),
                agent_bindings: BTreeMap::from([(agent_name.clone(), invalid_binding)]),
            },
        )]),
    );
    let context = DeploymentContext {
        environment: test_environment(),
        components: BTreeMap::from([(component.component_name.clone(), component)]),
        http_api_deployments: BTreeMap::new(),
        mcp_deployments: BTreeMap::new(),
        registered_agent_types: HashMap::from([(agent_name, agent_type)]),
    };
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    let compiled = context.compile_tools(
        golem_common::model::deployment::DeploymentRevision::INITIAL,
        &mut errors,
        &mut warnings,
    );

    assert_eq!(compiled.registered_tools.len(), 1);
    assert!(compiled.agent_tool_bindings.is_empty());
    assert!(warnings.is_empty());
    assert_eq!(
        errors
            .iter()
            .filter(|error| matches!(
                error,
                DeployValidationError::ToolBindingVersionMismatch { .. }
            ))
            .count(),
        2
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| matches!(
                error,
                DeployValidationError::ToolBindingAccountMismatch { .. }
            ))
            .count(),
        2
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| matches!(
                error,
                DeployValidationError::ToolBindingParametersMustBeObject { .. }
            ))
            .count(),
        2
    );
}

#[test]
fn compile_tools_accumulates_binding_errors_for_unknown_agent() {
    let tool_name = ToolName::try_from("grep").unwrap();
    let unknown_agent = AgentTypeName("MissingAgent".to_string());
    let invalid_binding = ToolBindingInput {
        version: Some("2.0.0".to_string()),
        parameters: NormalizedJsonValue::new(json!(["not", "an", "object"])),
        account: Some(AccountEmail::new("other@example.com")),
        config_keys_readable: Default::default(),
        secret_keys_readable: SecretKeyScope::All,
        secret_keys_revealable: SecretKeyScope::All,
        ..ToolBindingInput::default()
    };
    let component = test_tool_component(
        "tools",
        BTreeMap::from([(
            tool_name,
            ToolDeploymentMetadata {
                definition: test_tool("grep"),
                provision: ToolProvisionConfig::default(),
                environment_binding: None,
                component_bindings: BTreeMap::new(),
                agent_bindings: BTreeMap::from([(unknown_agent, invalid_binding)]),
            },
        )]),
    );
    let context = DeploymentContext {
        environment: test_environment(),
        components: BTreeMap::from([(component.component_name.clone(), component)]),
        http_api_deployments: BTreeMap::new(),
        mcp_deployments: BTreeMap::new(),
        registered_agent_types: HashMap::new(),
    };
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    context.compile_tools(
        golem_common::model::deployment::DeploymentRevision::INITIAL,
        &mut errors,
        &mut warnings,
    );

    assert!(
        errors
            .iter()
            .any(|error| matches!(error, DeployValidationError::ToolBindingUnknownAgent { .. }))
    );
    assert!(errors.iter().any(|error| matches!(
        error,
        DeployValidationError::ToolBindingVersionMismatch { .. }
    )));
    assert!(errors.iter().any(|error| matches!(
        error,
        DeployValidationError::ToolBindingAccountMismatch { .. }
    )));
    assert!(errors.iter().any(|error| matches!(
        error,
        DeployValidationError::ToolBindingParametersMustBeObject { .. }
    )));
}

#[test]
fn compile_tools_rejects_duplicate_implementations() {
    let tool_name = ToolName::try_from("grep").unwrap();
    let metadata = ToolDeploymentMetadata {
        definition: test_tool(tool_name.as_str()),
        provision: ToolProvisionConfig::default(),
        environment_binding: None,
        component_bindings: BTreeMap::new(),
        agent_bindings: BTreeMap::new(),
    };
    let first = test_tool_component(
        "tools-a",
        BTreeMap::from([(tool_name.clone(), metadata.clone())]),
    );
    let second = test_tool_component("tools-b", BTreeMap::from([(tool_name.clone(), metadata)]));
    let context = DeploymentContext {
        environment: test_environment(),
        components: BTreeMap::from([
            (first.component_name.clone(), first),
            (second.component_name.clone(), second),
        ]),
        http_api_deployments: BTreeMap::new(),
        mcp_deployments: BTreeMap::new(),
        registered_agent_types: HashMap::new(),
    };
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    let compiled = context.compile_tools(
        golem_common::model::deployment::DeploymentRevision::INITIAL,
        &mut errors,
        &mut warnings,
    );

    assert!(compiled.registered_tools.is_empty());
    assert!(compiled.agent_tool_bindings.is_empty());
    assert!(warnings.is_empty());
    assert!(matches!(
        errors.as_slice(),
        [DeployValidationError::DuplicateToolImplementation {
            tool_name: duplicate,
            components,
        }] if duplicate == &tool_name && components.len() == 2
    ));
}

#[test]
fn compile_tools_accumulates_binding_errors_for_duplicate_implementations() {
    let tool_name = ToolName::try_from("grep").unwrap();
    let unknown_agent = AgentTypeName("MissingAgent".to_string());
    let invalid_binding = ToolBindingInput {
        version: Some("2.0.0".to_string()),
        parameters: NormalizedJsonValue::new(json!(["not", "an", "object"])),
        account: Some(AccountEmail::new("other@example.com")),
        config_keys_readable: Default::default(),
        secret_keys_readable: SecretKeyScope::All,
        secret_keys_revealable: SecretKeyScope::All,
        ..ToolBindingInput::default()
    };
    let metadata = ToolDeploymentMetadata {
        definition: test_tool(tool_name.as_str()),
        provision: ToolProvisionConfig::default(),
        environment_binding: None,
        component_bindings: BTreeMap::new(),
        agent_bindings: BTreeMap::from([(unknown_agent, invalid_binding)]),
    };
    let first = test_tool_component(
        "tools-a",
        BTreeMap::from([(tool_name.clone(), metadata.clone())]),
    );
    let second = test_tool_component("tools-b", BTreeMap::from([(tool_name, metadata)]));
    let context = DeploymentContext {
        environment: test_environment(),
        components: BTreeMap::from([
            (first.component_name.clone(), first),
            (second.component_name.clone(), second),
        ]),
        http_api_deployments: BTreeMap::new(),
        mcp_deployments: BTreeMap::new(),
        registered_agent_types: HashMap::new(),
    };
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    context.compile_tools(
        golem_common::model::deployment::DeploymentRevision::INITIAL,
        &mut errors,
        &mut warnings,
    );

    assert!(errors.iter().any(|error| matches!(
        error,
        DeployValidationError::DuplicateToolImplementation { .. }
    )));
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, DeployValidationError::ToolBindingUnknownAgent { .. }))
    );
    assert!(errors.iter().any(|error| matches!(
        error,
        DeployValidationError::ToolBindingVersionMismatch { .. }
    )));
    assert!(errors.iter().any(|error| matches!(
        error,
        DeployValidationError::ToolBindingAccountMismatch { .. }
    )));
    assert!(errors.iter().any(|error| matches!(
        error,
        DeployValidationError::ToolBindingParametersMustBeObject { .. }
    )));
}

#[test]
fn compile_tools_accumulates_binding_errors_for_name_mismatched_definition() {
    let tool_name = ToolName::try_from("grep").unwrap();
    let unknown_agent = AgentTypeName("MissingAgent".to_string());
    let invalid_binding = ToolBindingInput {
        version: Some("2.0.0".to_string()),
        parameters: NormalizedJsonValue::new(json!(["not", "an", "object"])),
        account: Some(AccountEmail::new("other@example.com")),
        config_keys_readable: Default::default(),
        secret_keys_readable: SecretKeyScope::All,
        secret_keys_revealable: SecretKeyScope::All,
        ..ToolBindingInput::default()
    };
    let component = test_tool_component(
        "tools",
        BTreeMap::from([(
            tool_name,
            ToolDeploymentMetadata {
                definition: test_tool("git"),
                provision: ToolProvisionConfig::default(),
                environment_binding: None,
                component_bindings: BTreeMap::new(),
                agent_bindings: BTreeMap::from([(unknown_agent, invalid_binding)]),
            },
        )]),
    );
    let context = DeploymentContext {
        environment: test_environment(),
        components: BTreeMap::from([(component.component_name.clone(), component)]),
        http_api_deployments: BTreeMap::new(),
        mcp_deployments: BTreeMap::new(),
        registered_agent_types: HashMap::new(),
    };
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    context.compile_tools(
        golem_common::model::deployment::DeploymentRevision::INITIAL,
        &mut errors,
        &mut warnings,
    );

    assert!(errors.iter().any(|error| matches!(
        error,
        DeployValidationError::ToolDefinitionNameMismatch { .. }
    )));
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, DeployValidationError::ToolBindingUnknownAgent { .. }))
    );
    assert!(errors.iter().any(|error| matches!(
        error,
        DeployValidationError::ToolBindingVersionMismatch { .. }
    )));
    assert!(errors.iter().any(|error| matches!(
        error,
        DeployValidationError::ToolBindingAccountMismatch { .. }
    )));
    assert!(errors.iter().any(|error| matches!(
        error,
        DeployValidationError::ToolBindingParametersMustBeObject { .. }
    )));
}

#[test]
fn compile_tool_binding_merges_parameters_and_narrows_revealable_secrets() {
    let readable_path = CanonicalAgentSecretPath(vec!["readable".to_string()]);
    let dropped_path = CanonicalAgentSecretPath(vec!["dropped".to_string()]);
    let environment = ToolBindingInput {
        version: None,
        parameters: NormalizedJsonValue::new(json!({
            "nested": { "environment": true },
            "environment": true
        })),
        account: None,
        config_keys_readable: Default::default(),
        secret_keys_readable: SecretKeyScope::Keys(BTreeSet::from([readable_path.clone()])),
        secret_keys_revealable: SecretKeyScope::Keys(BTreeSet::from([
            readable_path.clone(),
            dropped_path,
        ])),
        ..ToolBindingInput::default()
    };
    let agent = ToolBindingInput {
        version: None,
        parameters: NormalizedJsonValue::new(json!({
            "nested": { "agent": true },
            "agent": true
        })),
        account: None,
        config_keys_readable: Default::default(),
        secret_keys_readable: SecretKeyScope::All,
        secret_keys_revealable: SecretKeyScope::All,
        ..ToolBindingInput::default()
    };
    let component = test_tool_component("tools", BTreeMap::new());
    let source = ToolSource::Component {
        component_id: component.id,
        component_revision: component.revision,
        component_name: component.component_name.clone(),
    };
    let mut warnings = Vec::new();

    let binding = compile_tool_binding(
        golem_common::model::deployment::DeploymentRevision::INITIAL,
        ToolBindingOwner::AgentType {
            agent_type_name: AgentTypeName("AgentA".to_string()),
        },
        &ToolName::try_from("grep").unwrap(),
        Some(&environment),
        Some(&agent),
        None,
        component.account_id,
        &component.account_email,
        source,
        "1.0.0",
        TOOL_METADATA_WIT_VERSION,
        Default::default(),
        &mut warnings,
    )
    .unwrap();

    assert_eq!(
        binding.parameters.0,
        json!({
            "nested": { "agent": true },
            "environment": true,
            "agent": true
        })
    );
    assert_eq!(
        binding.secret_keys_readable,
        SecretKeyScope::Keys(BTreeSet::from([readable_path.clone()]))
    );
    assert_eq!(
        binding.secret_keys_revealable,
        SecretKeyScope::Keys(BTreeSet::from([readable_path]))
    );
    assert!(matches!(
        warnings.as_slice(),
        [
            crate::services::deployment::DeployValidationWarning::ToolRevealableSecretKeysDropped(
                _
            )
        ]
    ));
}

#[test]
fn secret_default_plaintext_value_is_parsed_against_secret_inner_type() {
    let path = CanonicalAgentSecretPath(vec!["apiKey".to_string()]);
    let default = DeploymentAgentSecretDefault {
        path: AgentSecretPath(vec!["apiKey".to_string()]),
        secret_value: json!("s3cr3t"),
    };
    let schema = SchemaGraph::anonymous(SchemaType::secret(SecretSpec {
        inner: Box::new(SchemaType::string()),
        category: None,
    }));
    let schema = stored_agent_secret_schema(&path, &schema, &schema.root)
        .expect("secret<T> config declarations should be accepted");

    let parsed = parse_default_secret_value(&path, Some(&&default), &schema)
        .expect("plaintext defaults for secret<T> should be parsed as T");

    assert_eq!(parsed, Some(SchemaValue::String("s3cr3t".to_string())));
}

#[test]
fn optional_secret_default_plaintext_value_is_parsed_against_secret_inner_type() {
    let path = CanonicalAgentSecretPath(vec!["apiKey".to_string()]);
    let default = DeploymentAgentSecretDefault {
        path: AgentSecretPath(vec!["apiKey".to_string()]),
        secret_value: json!("s3cr3t"),
    };
    let schema = SchemaGraph::anonymous(SchemaType::option(SchemaType::secret(SecretSpec {
        inner: Box::new(SchemaType::string()),
        category: None,
    })));
    let schema = stored_agent_secret_schema(&path, &schema, &schema.root)
        .expect("option<secret<T>> config declarations should be accepted");

    let parsed = parse_default_secret_value(&path, Some(&&default), &schema)
        .expect("plaintext defaults for option<secret<T>> should be parsed as T");

    assert_eq!(parsed, Some(SchemaValue::String("s3cr3t".to_string())));
}

#[test]
fn non_secret_config_declaration_is_rejected() {
    let agent_type_name = AgentTypeName("vault".to_string());
    let config_path = vec!["apiKey".to_string()];
    let agent_type = agent_type_with_secret_config(
        agent_type_name.clone(),
        config_path.clone(),
        SchemaType::string(),
    );
    let mut registered_agent_types = HashMap::new();
    registered_agent_types.insert(
        agent_type_name,
        InProgressDeployedRegisteredAgentType {
            agent_type,
            implemented_by: test_implementer(),
            webhook_domain_and_segments: None,
        },
    );
    let context = DeploymentContext {
        environment: test_environment(),
        components: BTreeMap::new(),
        http_api_deployments: BTreeMap::new(),
        mcp_deployments: BTreeMap::new(),
        registered_agent_types,
    };
    let mut errors = Vec::new();

    let (creations, updates, replacements) = context.deployment_agent_secret_creations_and_updates(
        Vec::new(),
        Vec::new(),
        false,
        &mut errors,
    );

    assert!(creations.is_empty());
    assert!(updates.is_empty());
    assert!(replacements.is_empty());
    assert_eq!(
        errors,
        vec![DeployValidationError::AgentSecretInvalidConfigType {
            path: CanonicalAgentSecretPath::from_path_in_unknown_casing(&config_path),
        }]
    );
}

#[test]
fn nested_secret_payload_config_declaration_is_rejected() {
    let path = CanonicalAgentSecretPath(vec!["apiKey".to_string()]);
    let schema = SchemaGraph::anonymous(SchemaType::secret(SecretSpec {
        inner: Box::new(SchemaType::secret(SecretSpec {
            inner: Box::new(SchemaType::string()),
            category: None,
        })),
        category: None,
    }));

    let result = stored_agent_secret_schema(&path, &schema, &schema.root);

    assert_eq!(
        result,
        Err(DeployValidationError::AgentSecretInvalidConfigType { path })
    );
}

#[test]
fn nested_quota_token_payload_config_declaration_is_rejected() {
    let path = CanonicalAgentSecretPath(vec!["quota".to_string()]);
    let schema = SchemaGraph::anonymous(SchemaType::option(SchemaType::secret(SecretSpec {
        inner: Box::new(SchemaType::quota_token(QuotaTokenSpec {
            resource_name: Some("credits".to_string()),
        })),
        category: None,
    })));

    let result = stored_agent_secret_schema(&path, &schema, &schema.root);

    assert_eq!(
        result,
        Err(DeployValidationError::AgentSecretInvalidConfigType { path })
    );
}

#[test]
fn referenced_secret_declaration_projects_away_outer_secret_def() {
    let path = CanonicalAgentSecretPath(vec!["apiKey".to_string()]);
    let outer_id = TypeId::new("api-key-secret");
    let inner_id = TypeId::new("api-key-inner");
    let schema = SchemaGraph {
        defs: vec![
            SchemaTypeDef {
                id: outer_id.clone(),
                name: None,
                body: SchemaType::secret(SecretSpec {
                    inner: Box::new(SchemaType::ref_to(inner_id.clone())),
                    category: None,
                }),
            },
            SchemaTypeDef {
                id: inner_id.clone(),
                name: None,
                body: SchemaType::string(),
            },
        ],
        root: SchemaType::string(),
    };

    let stored_schema = stored_agent_secret_schema(&path, &schema, &SchemaType::ref_to(outer_id))
        .expect("ref to secret<T> should be accepted and stored as plaintext T");

    assert!(matches!(stored_schema.root, SchemaType::Ref { .. }));
    assert_eq!(stored_schema.defs.len(), 1);
    assert_eq!(stored_schema.defs[0].id, inner_id);
    assert!(matches!(
        stored_schema.defs[0].body,
        SchemaType::String { .. }
    ));
}

#[test]
fn optional_secret_default_creation_stores_plaintext_inner_schema_not_option_schema() {
    let agent_type_name = AgentTypeName("vault".to_string());
    let config_path = vec!["apiKey".to_string()];
    let agent_type = agent_type_with_secret_config(
        agent_type_name.clone(),
        config_path.clone(),
        SchemaType::option(SchemaType::secret(SecretSpec {
            inner: Box::new(SchemaType::string()),
            category: None,
        })),
    );
    let mut registered_agent_types = HashMap::new();
    registered_agent_types.insert(
        agent_type_name,
        InProgressDeployedRegisteredAgentType {
            agent_type,
            implemented_by: test_implementer(),
            webhook_domain_and_segments: None,
        },
    );
    let context = DeploymentContext {
        environment: test_environment(),
        components: BTreeMap::new(),
        http_api_deployments: BTreeMap::new(),
        mcp_deployments: BTreeMap::new(),
        registered_agent_types,
    };
    let default = DeploymentAgentSecretDefault {
        path: AgentSecretPath(config_path),
        secret_value: json!("s3cr3t"),
    };
    let mut errors = Vec::new();

    let (creations, updates, replacements) = context.deployment_agent_secret_creations_and_updates(
        Vec::new(),
        vec![default],
        false,
        &mut errors,
    );

    assert!(
        errors.is_empty(),
        "unexpected validation errors: {errors:?}"
    );
    assert_eq!(updates.len(), 0);
    assert_eq!(replacements.len(), 0);
    assert_eq!(creations.len(), 1);
    assert_eq!(
        creations[0].secret_value,
        Some(SchemaValue::String("s3cr3t".to_string()))
    );
    match resolve_schema_ref(&creations[0].secret_type, &creations[0].secret_type.root) {
        SchemaType::String { .. } => {}
        other => {
            panic!("deployment-created agent secrets must be stored as plaintext T, not {other:?}")
        }
    }
}

#[test]
fn optional_secret_declaration_accepts_existing_stored_plaintext_schema() {
    let agent_type_name = AgentTypeName("vault".to_string());
    let config_path = vec!["apiKey".to_string()];
    let agent_type = agent_type_with_secret_config(
        agent_type_name.clone(),
        config_path.clone(),
        SchemaType::option(SchemaType::secret(SecretSpec {
            inner: Box::new(SchemaType::string()),
            category: None,
        })),
    );
    let mut registered_agent_types = HashMap::new();
    registered_agent_types.insert(
        agent_type_name,
        InProgressDeployedRegisteredAgentType {
            agent_type,
            implemented_by: test_implementer(),
            webhook_domain_and_segments: None,
        },
    );
    let context = DeploymentContext {
        environment: test_environment(),
        components: BTreeMap::new(),
        http_api_deployments: BTreeMap::new(),
        mcp_deployments: BTreeMap::new(),
        registered_agent_types,
    };
    let existing_secret = stored_agent_secret(
        &config_path,
        SchemaGraph::anonymous(SchemaType::string()),
        Some(SchemaValue::String("already-set".to_string())),
    );
    let mut errors = Vec::new();

    let (creations, updates, replacements) = context.deployment_agent_secret_creations_and_updates(
        vec![existing_secret],
        Vec::new(),
        false,
        &mut errors,
    );

    assert!(
        errors.is_empty(),
        "option<secret<T>> declarations should accept existing stored plaintext T; got {errors:?}"
    );
    assert_eq!(creations.len(), 0);
    assert_eq!(updates.len(), 0);
    assert_eq!(replacements.len(), 0);
}
