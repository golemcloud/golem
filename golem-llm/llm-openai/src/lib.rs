wit_bindgen::generate!({
    world: "libllm",
    path: "../llm.wit",
});

mod api;
mod types;

use golem_rust::use_persistence_level;

use crate::exports::golem::llm::llm;

struct Component;

impl llm::Guest for Component {
    type ChatStream = types::ChatStream;

    fn send(
        messages: Vec<llm::Message>,
        config: llm::Config,
    ) -> Result<llm::ChatEvent, llm::Error> {
        let api_key = std::env::var("OPENAI_API_KEY").map_err(|_| llm::Error {
            code: llm::ErrorCode::InvalidRequest,
            message: "OPENAI_API_KEY is missing".to_owned(),
            provider_error_json: None,
        })?;
        let openai = api::OpenAI::new(api_key);

        let _guard = use_persistence_level(golem_rust::PersistenceLevel::PersistRemoteSideEffects);
        openai.generate_completions(messages, vec![], config)
    }

    fn continue_inference(
        messages: Vec<llm::Message>,
        tool_results: Vec<llm::ToolResult>,
        config: llm::Config,
    ) -> Result<llm::ChatEvent, llm::Error> {
        let api_key = std::env::var("OPENAI_API_KEY").map_err(|_| llm::Error {
            code: llm::ErrorCode::InvalidRequest,
            message: "OPENAI_API_KEY is missing".to_owned(),
            provider_error_json: None,
        })?;
        let openai = api::OpenAI::new(api_key);

        let _guard = use_persistence_level(golem_rust::PersistenceLevel::PersistRemoteSideEffects);
        openai.generate_completions(messages, tool_results, config)
    }

    fn stream_chat(
        messages: Vec<llm::Message>,
        config: llm::Config,
    ) -> Result<llm::ChatStream, llm::Error> {
        let api_key = std::env::var("OPENAI_API_KEY").map_err(|_| llm::Error {
            code: llm::ErrorCode::InvalidRequest,
            message: "OPENAI_API_KEY is missing".to_owned(),
            provider_error_json: None,
        })?;
        let openai = api::OpenAI::new(api_key);

        let _guard = use_persistence_level(golem_rust::PersistenceLevel::PersistRemoteSideEffects);
        openai.stream_completions(messages, config).map(llm::ChatStream::new)
    }
}

export!(Component);
