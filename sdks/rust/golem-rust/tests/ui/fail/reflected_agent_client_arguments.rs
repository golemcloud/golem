use golem_rust::agent_client;

#[agent_client(type_name = "SearchAgent")]
trait SearchApi {
    fn status(&self) -> String;
}

fn main() {}
