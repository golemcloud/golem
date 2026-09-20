use golem_rust::agentic::{
    AgentTypeName, EnrichedAgentMethod, EnrichedParameterSchema, ExtendedAgentConstructor,
    ExtendedAgentType, register_agent_type,
};
use golem_rust::golem_agentic::golem::agent::common::{
    AgentMode, AgentTypeKind, AuthDetails, CorsOptions, ExactFileMapping, FileMapping,
    HttpMountDetails, PathSegment, PathVariable, Snapshotting, SubtreeFileMapping,
};
use golem_rust::{agent_definition, agent_implementation};

#[agent_definition(snapshotting = "disabled")]
pub trait LiveFiles {
    fn new(name: String) -> Self;
    fn replace(&self, path: String, contents: Vec<u8>) -> bool;
}

struct LiveFilesImpl;

#[agent_implementation]
impl LiveFiles for LiveFilesImpl {
    fn __register_agent_type() {
        let string = || {
            EnrichedParameterSchema::Value(
                golem_rust::schema::try_into_schema_graph::<String>().unwrap(),
            )
        };
        register_agent_type(
            AgentTypeName("LiveFiles".into()),
            ExtendedAgentType {
                type_name: "LiveFiles".into(),
                kind: AgentTypeKind::Regular,
                description: "Live filesystem HTTP test fixture".into(),
                source_language: "rust".into(),
                constructor: ExtendedAgentConstructor {
                    name: None,
                    description: String::new(),
                    prompt_hint: None,
                    input_schema: vec![("name".into(), string())],
                },
                methods: vec![EnrichedAgentMethod {
                    name: "replace".into(),
                    description: String::new(),
                    http_endpoint: vec![],
                    prompt_hint: None,
                    input_schema: vec![
                        ("path".into(), string()),
                        (
                            "contents".into(),
                            EnrichedParameterSchema::Value(
                                golem_rust::schema::try_into_schema_graph::<Vec<u8>>().unwrap(),
                            ),
                        ),
                    ],
                    output_schema: vec![(
                        "ok".into(),
                        golem_rust::schema::try_into_schema_graph::<bool>().unwrap(),
                    )],
                    read_only: None,
                }],
                dependencies: vec![],
                mode: AgentMode::Durable,
                http_mount: Some(HttpMountDetails {
                    path_prefix: vec![
                        PathSegment::Literal("files".into()),
                        PathSegment::PathVariable(PathVariable {
                            variable_name: "name".into(),
                        }),
                    ],
                    auth_details: Some(AuthDetails { required: false }),
                    phantom_agent: false,
                    cors_options: CorsOptions {
                        allowed_patterns: vec!["https://allowed.test".into()],
                    },
                    webhook_suffix: vec![],
                    static_bindings: vec![],
                    filesystem_bindings: vec![
                        FileMapping::Exact(ExactFileMapping {
                            public_path: vec!["alias".into()],
                            file_path: "/public/created.txt".into(),
                        }),
                        FileMapping::Subtree(SubtreeFileMapping {
                            public_prefix: vec![],
                            filesystem_root: "/public".into(),
                        }),
                        FileMapping::Subtree(SubtreeFileMapping {
                            public_prefix: vec![],
                            filesystem_root: "/fallback".into(),
                        }),
                    ],
                    openapi_provider_method: None,
                }),
                snapshotting: Snapshotting::Disabled,
                config: vec![],
                sorted_method_indices: vec![],
            },
        );
    }

    fn new(name: String) -> Self {
        assert!(name != "fail", "private initializer failure");
        std::fs::create_dir_all("/public/directory").unwrap();
        std::fs::create_dir_all("/fallback").unwrap();
        std::fs::create_dir_all("/private").unwrap();
        let (root, _) = wasi::filesystem::preopens::get_directories()
            .into_iter()
            .find(|(_, path)| path == "/")
            .unwrap();
        std::fs::write("/public/created.txt", name.as_bytes()).unwrap();
        std::fs::write("/fallback/only.txt", b"fallback").unwrap();
        std::fs::write("/fallback/directory", b"must not fall through").unwrap();
        std::fs::write("/private/secret.txt", b"must not be exposed").unwrap();
        root.symlink_at("../private/secret.txt", "public/link.txt")
            .unwrap();
        root.symlink_at("../private", "public/parent-link").unwrap();
        if name.starts_with("large-") {
            // gRPC gzip must not shrink this below the transport receive window.
            let mut contents = vec![0; 32 * 1024 * 1024];
            blake3::Hasher::new()
                .update(b"live-file-backpressure")
                .finalize_xof()
                .fill(&mut contents);
            std::fs::write("/public/large.bin", contents).unwrap();
        }
        Self
    }

    fn replace(&self, path: String, contents: Vec<u8>) -> bool {
        std::fs::write(path, contents).is_ok()
    }
}
