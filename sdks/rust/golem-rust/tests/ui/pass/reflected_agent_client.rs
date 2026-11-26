use golem_rust::agentic::AgentStream;
use golem_rust::{MethodOnlyAgentClientDefinition, ConfigSchema, IntoSchema, agent_client};

#[derive(IntoSchema)]
struct Query {
    text: String,
}

#[derive(ConfigSchema)]
struct SearchConfig {
    model: String,
}

#[agent_client]
trait SearchApi {
    fn status(&self) -> String;
    fn search(&self, query: Query, limit: u32) -> Vec<String>;
    fn reschedule(&self, at: String);
    fn schedule_raw(&self, r#at: String);
    fn schedule_mixed(&self, at: String, r#at1: String);
    fn transform(&self, input: AgentStream<String>) -> AgentStream<String>;
    fn invalidate(&self, key: String);
}

#[agent_client(type_name = "SearchAgent")]
trait CompleteSearchApi {
    fn new(
        namespace: String,
        #[agent_config] config: golem_rust::agentic::Config<SearchConfig>,
    ) -> Self;
    fn status(&self) -> String;
}

#[agent_client(type_name = "SearchRequest", mode = "ephemeral")]
trait EphemeralSearchApi {
    fn new(namespace: String) -> Self;
    fn status(&self) -> String;
}

fn definition_is_caller_owned() -> MethodOnlyAgentClientDefinition {
    SearchApiClient::client_definition().expect("valid client definition")
}

async fn full_clients(agent_id: &golem_rust::AgentId) {
    let complete = CompleteSearchApiClient::upsert("default".to_string())
        .expect("fallible full client creation");
    let _status = complete.status();
    let _: Result<golem_rust::Invocation<String>, golem_rust::GolemReflectError> =
        complete.trigger_status().await;
    let _: Result<golem_rust::AgentId, golem_rust::GolemReflectError> = complete.agent_id().await;
    let _ = CompleteSearchApiClient::create(
        "default".to_string(),
        golem_rust::CreateOptions {
            keyed: true,
            config: SearchConfigRpc {
                model: Some("fast".to_string()),
            },
            env: vec![("MODE".to_string(), "test".to_string())],
        },
    )
    .await
    .expect("fallible keyed creation");
    let _ = CompleteSearchApiClient::create("default".to_string(), Default::default())
        .await
        .expect("fallible creation");
    let _ = CompleteSearchApiClient::get_by_id(agent_id).expect("fallible lookup by id");
    let ephemeral = EphemeralSearchApiClient::upsert("default".to_string())
        .expect("fallible ephemeral creation");
    let _status = ephemeral.status();
    let _: Result<golem_rust::Invocation<String>, golem_rust::GolemReflectError> =
        ephemeral.trigger_status().await;
    let method_only = SearchApiClient::get_by_id(agent_id).expect("method-only client");
    let _: Result<golem_rust::AgentId, golem_rust::GolemReflectError> =
        method_only.agent_id().await;
}

fn main() {
    let _ = definition_is_caller_owned();
    let _ = full_clients;

    let durable = MethodOnlyAgentClientDefinition::builder()
        .durable::<Query>("SearchAgent")
        .method::<Query, Vec<String>>("search")
        .expect("method schema")
        .build();
    let ephemeral = MethodOnlyAgentClientDefinition::builder()
        .ephemeral::<Query>("SearchRequest")
        .method::<Query, Vec<String>>("search")
        .expect("method schema")
        .build();
    let configured = MethodOnlyAgentClientDefinition::builder()
        .durable::<Query>("SearchAgent")
        .config::<SearchConfig>()
        .build();

    if false {
        let constructor = Query {
            text: "query".to_string(),
        };
        let agent_id = golem_rust::AgentId::new(golem_rust::Uuid::nil());
        let client = durable.get_by_id(&agent_id).expect("durable get by id");
        let _ = client.agent_id();
        let _ = durable.upsert(&constructor);
        let _ = durable.create(&constructor, golem_rust::CreateOptions::default());
        let _ = ephemeral.upsert(&constructor);
        let config = SearchConfigRpc {
            model: Some("fast".to_string()),
        };
        let _ = configured.create(
            &constructor,
            golem_rust::CreateOptions {
                keyed: true,
                config,
                env: Vec::new(),
            },
        );
    }
}
