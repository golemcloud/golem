use golem_rust::agentic::{
    AgentInitiator, AgentInvocationResult, AgentTypeName, BaseAgent, DirectAgentInput,
    EnrichedAgentMethod, EnrichedParameterSchema, ExtendedAgentConstructor, ExtendedAgentType,
    Principal, ResolvedAgent, Schema, SnapshotData, SnapshotRestoreContext, custom_error,
    get_agent_id, get_agent_type_by_name, register_agent_initiator, register_agent_type,
};
use golem_rust::golem_agentic::golem::agent::common::{
    AgentError, AgentMode, AgentType, AgentTypeKind, Snapshotting,
};
use golem_rust::schema::wit::{direct, wire::SchemaValueTree};

struct MethodFailure {
    count: i64,
    type_name: &'static str,
}

impl MethodFailure {
    fn new(type_name: &'static str) -> Self {
        Self {
            count: 0,
            type_name,
        }
    }
    fn fail(&mut self) -> i64 {
        self.count += 7;
        self.count
    }
    fn value(&self) -> i64 {
        self.count
    }
    fn increment(&mut self) -> i64 {
        self.count += 1;
        self.count
    }
    fn declared_error(&self) -> Result<i64, String> {
        Err("declared error".into())
    }
    fn panic(&self) -> i64 {
        panic!("method panicked")
    }
}

// The Rust macro encodes Result as a declared value. Use the SDK's BaseAgent
// error channel explicitly to distinguish it from a declared Result and a trap.
#[golem_rust::async_trait::async_trait(?Send)]
impl BaseAgent for MethodFailure {
    fn get_agent_id(&self) -> String {
        get_agent_id().agent_id
    }

    async fn invoke(
        &mut self,
        method_name: String,
        input: SchemaValueTree,
        _principal: Principal,
    ) -> Result<AgentInvocationResult, AgentError> {
        let mut input = DirectAgentInput::new(input).map_err(custom_error)?;
        if method_name == "fail_after_http" {
            let url = input.take::<String>().map_err(custom_error)?;
            input.finish().map_err(custom_error)?;
            let request = golem_rust::http::Request::post(url)
                .body(Vec::new())
                .map_err(custom_error)?;
            let response = golem_rust::http::send(request)
                .await
                .map_err(custom_error)?;
            golem_rust::http::response_bytes(response)
                .await
                .map_err(custom_error)?;
            let _ = std::time::SystemTime::now();
            self.fail();
            return Err(custom_error("method gave up"));
        }
        input.finish().map_err(custom_error)?;
        let value = match method_name.as_str() {
            "fail" => {
                self.fail();
                return Err(custom_error("method gave up"));
            }
            "value" => direct::encode_async(&self.value()).await,
            "increment" => direct::encode_async(&self.increment()).await,
            "declared_error" => direct::encode_async(&self.declared_error()).await,
            "panic" => direct::encode_async(&self.panic()).await,
            _ => return Err(golem_rust::agentic::invalid_method_error(method_name)),
        }
        .map_err(custom_error)?;
        Ok(AgentInvocationResult { value: Some(value) })
    }

    fn get_definition(&self) -> AgentType {
        get_agent_type_by_name(&AgentTypeName(self.type_name.into())).unwrap()
    }

    async fn save_snapshot_base(&self) -> Result<SnapshotData, String> {
        Err("snapshots disabled".into())
    }
}

struct Initiator(&'static str);

#[golem_rust::async_trait::async_trait(?Send)]
impl AgentInitiator for Initiator {
    async fn initiate(
        &self,
        params: SchemaValueTree,
        _principal: Principal,
    ) -> Result<ResolvedAgent, AgentError> {
        let mut input = DirectAgentInput::new(params).map_err(custom_error)?;
        let name = input.take::<String>().map_err(custom_error)?;
        input.finish().map_err(custom_error)?;
        if name == "fail-constructor" {
            return Err(custom_error("constructor gave up"));
        }
        Ok(ResolvedAgent::new(Box::new(MethodFailure::new(self.0))))
    }

    async fn restore(
        &self,
        _snapshot: SnapshotData,
        _context: SnapshotRestoreContext,
    ) -> Result<ResolvedAgent, String> {
        Err("snapshots disabled".into())
    }
}

golem_rust::ctor::__support::ctor_parse!(
    #[ctor]
    fn register_method_failure() {
        golem_rust::agentic::install_agent_exports();
        for (type_name, mode) in [
            ("RustMethodFailure", AgentMode::Durable),
            ("RustEphemeralMethodFailure", AgentMode::Ephemeral),
        ] {
            register_agent_type(
                AgentTypeName(type_name.into()),
                ExtendedAgentType {
                    type_name: type_name.into(),
                    kind: AgentTypeKind::Regular,
                    description: String::new(),
                    source_language: "rust".into(),
                    constructor: ExtendedAgentConstructor {
                        name: None,
                        description: String::new(),
                        prompt_hint: None,
                        input_schema: vec![(
                            "name".into(),
                            EnrichedParameterSchema::Value(
                                String::get_type().get_schema_graph().unwrap(),
                            ),
                        )],
                    },
                    methods: [
                        "fail",
                        "fail_after_http",
                        "value",
                        "increment",
                        "declared_error",
                        "panic",
                    ]
                    .into_iter()
                    .map(|name| EnrichedAgentMethod {
                        name: name.into(),
                        description: String::new(),
                        prompt_hint: None,
                        http_endpoint: vec![],
                        input_schema: if name == "fail_after_http" {
                            vec![(
                                "url".into(),
                                EnrichedParameterSchema::Value(
                                    String::get_type().get_schema_graph().unwrap(),
                                ),
                            )]
                        } else {
                            vec![]
                        },
                        output_schema: vec![(
                            "return_value".into(),
                            if name == "declared_error" {
                                <Result<i64, String>>::get_type()
                                    .get_schema_graph()
                                    .unwrap()
                            } else {
                                i64::get_type().get_schema_graph().unwrap()
                            },
                        )],
                        read_only: None,
                    })
                    .collect(),
                    sorted_method_indices: vec![],
                    dependencies: vec![],
                    mode,
                    http_mount: None,
                    snapshotting: Snapshotting::Disabled,
                    config: vec![],
                },
            );
            register_agent_initiator(type_name, std::sync::Arc::new(Initiator(type_name)));
        }
    }
);
