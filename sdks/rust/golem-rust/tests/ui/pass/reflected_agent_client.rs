use golem_rust::agentic::AgentStream;
use golem_rust::{AgentClientDefinition, ConfigSchema, IntoSchema, agent_client};

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

fn definition_is_caller_owned() -> AgentClientDefinition {
    SearchApiClient::client_definition().expect("valid client definition")
}

fn main() {
    let _ = definition_is_caller_owned();
    if false {
        let _ = CompleteSearchApiClient::agent_id("default".to_string(), None);
        let _ = CompleteSearchApiClient::get("default".to_string());
        let _ = CompleteSearchApiClient::new_phantom("default".to_string());
        let _ = CompleteSearchApiClient::get_phantom(
            golem_rust::Uuid::new_v4(),
            "default".to_string(),
        );
        let _ = EphemeralSearchApiClient::new_phantom("default".to_string());
        let _ = EphemeralSearchApiClient::get_phantom(
            golem_rust::Uuid::new_v4(),
            "default".to_string(),
        );
    }

    let durable = AgentClientDefinition::builder()
        .durable::<Query>("SearchAgent")
        .method::<Query, Vec<String>>("search")
        .expect("method schema")
        .build();
    let ephemeral = AgentClientDefinition::builder()
        .ephemeral::<Query>("SearchRequest")
        .method::<Query, Vec<String>>("search")
        .expect("method schema")
        .build();
    let configured = AgentClientDefinition::builder()
        .durable::<Query>("SearchAgent")
        .config::<SearchConfig>()
        .build();

    if false {
        let constructor = Query {
            text: "query".to_string(),
        };
        let phantom_id = golem_rust::Uuid::new_v4();
        let parsed = durable
            .agent_id(&constructor, Some(phantom_id))
            .expect("agent id");
        let client = durable.bind(&parsed).expect("durable bind");
        let _ = client.agent_id();
        let _ = durable.get(&constructor);
        let _ = durable.get_phantom(phantom_id, &constructor);
        let _ = durable.new_phantom(&constructor);
        let _ = ephemeral.get_phantom(phantom_id, &constructor);
        let _ = ephemeral.new_phantom(&constructor);
        let config = SearchConfigRpc {
            model: Some("fast".to_string()),
        };
        let _ = configured.get_with_config(&constructor, config);
    }
}
