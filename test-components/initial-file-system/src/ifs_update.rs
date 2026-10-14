use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
pub trait IfsUpdate {
    fn new(name: String) -> Self;
    fn load_file(&mut self);
    fn get_file_content(&self) -> String;
}

struct IfsUpdateImpl {
    _name: String,
    content: String,
}

#[agent_implementation]
impl IfsUpdate for IfsUpdateImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            content: String::new(),
        }
    }

    fn load_file(&mut self) {
        self.content = std::fs::read_to_string("/foo.txt").unwrap();
    }

    fn get_file_content(&self) -> String {
        self.content.clone()
    }

    // intentionally incorrect implementation
    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }

    // intentionally incorrect implementation
    async fn load_snapshot(
        _bytes: Vec<u8>,
        context: golem_rust::agentic::SnapshotRestoreContext,
    ) -> Result<Self, String> {
        let mut parameters = golem_rust::agentic::DirectAgentInput::new(context.parameters)
            .map_err(|e| e.to_string())?;
        let name = parameters.take::<String>().map_err(|e| e.to_string())?;
        parameters.finish().map_err(|e| e.to_string())?;
        Ok(Self {
            _name: name,
            content: "restored".to_string(),
        })
    }
}
