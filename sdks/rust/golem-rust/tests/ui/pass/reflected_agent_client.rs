use golem_rust::{AgentClientDefinition, IntoSchema, agent_client};

#[derive(IntoSchema)]
struct Query {
    text: String,
}

#[agent_client(type_name = "SearchAgent")]
trait SearchApi {
    fn status(&self) -> String;
    fn search(&self, query: Query, limit: u32) -> Vec<String>;
    fn invalidate(&self, key: String);
}

fn definition_is_caller_owned() -> AgentClientDefinition {
    SearchApiClient::client_definition().expect("valid client definition")
}

fn main() {
    let _ = definition_is_caller_owned();
}
