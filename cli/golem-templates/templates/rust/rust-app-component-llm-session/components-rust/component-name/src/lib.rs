use golem_rust::golem_ai::golem::llm::llm;
use golem_rust::golem_ai::golem::llm::llm::{
    Config, ContentPart, Event, Message, Response, Role, ToolResult,
};
use golem_rust::{agent_definition, agent_implementation, description};

#[agent_definition]
pub trait ChatAgent {
    fn new(chat_name: String) -> Self;

    #[description("Ask questions")]
    async fn ask(&mut self, question: String) -> String;
    async fn history(&self) -> Vec<Event>;
}

struct ChatAgentImpl {
    chat_name: String,
    session: LlmSession,
}

#[agent_implementation]
impl ChatAgent for ChatAgentImpl {
    fn new(chat_name: String) -> Self {
        let mut result = Self {
            chat_name,
            session: LlmSession::new(Config {
                model: "gpt-oss:20b".to_string(),
                temperature: None,
                max_tokens: None,
                stop_sequences: None,
                tools: None,
                tool_choice: None,
                provider_options: None,
            }),
        };
        result.session.add_message(Message {
            role: Role::System,
            name: None,
            content: vec![ContentPart::Text(format!(
                "You are a helpful and very funny assistant for a chat named {}",
                result.chat_name
            ))],
        });
        result
    }

    async fn ask(&mut self, question: String) -> String {
        self.session.add_message(Message {
            role: Role::User,
            name: None,
            content: vec![ContentPart::Text(question)],
        });
        let response = self.session.send();
        response
            .content
            .iter()
            .filter_map(|content_part| match content_part {
                ContentPart::Text(txt) => Some(txt.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    async fn history(&self) -> Vec<Event> {
        self.session.events.clone()
    }
}

pub struct LlmSession {
    pub config: Config,
    pub events: Vec<Event>,
}

impl LlmSession {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            events: Vec::new(),
        }
    }

    pub fn add_message(&mut self, message: Message) {
        self.events.push(Event::Message(message));
    }

    pub fn add_tool_result(&mut self, tool_result: ToolResult) {
        self.events.push(Event::ToolResults(vec![tool_result]));
    }

    pub fn add_tool_results(&mut self, tool_results: Vec<ToolResult>) {
        self.events.push(Event::ToolResults(tool_results));
    }

    pub fn send(&mut self) -> Response {
        let response =
            llm::send(&self.events, &self.config).expect("Failed to send message to LLM");
        self.events.push(Event::Response(response.clone()));
        response
    }
}
