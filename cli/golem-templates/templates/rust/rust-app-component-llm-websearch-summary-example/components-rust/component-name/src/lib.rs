use golem_llm_openai::golem::llm::llm;
use golem_llm_openai::golem::llm::llm::{Config, ContentPart, Message, Role};
use golem_web_search_google::golem::web_search::types;
use golem_web_search_google::golem::web_search::web_search;
use golem_rust::{agent_definition, agent_implementation, description, Schema, endpoint};

#[derive(Clone, Schema, serde::Serialize, serde::Deserialize)]
pub struct SearchResult {
    url: String,
    title: String,
    snippet: String,
}

#[agent_definition(
    mount = "/research",
    phantom_agent = true
)]
pub trait ResearchAgent {
    fn new() -> Self;

    #[description("Research and summarize a topic")]
    #[endpoint(get = "/?topic={topic}")]
    fn research(&self, topic: String) -> String;
}

struct ResearchAgentImpl {
    model: String,
}

#[agent_implementation]
impl ResearchAgent for ResearchAgentImpl {
    fn new() -> Self {
        let model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "gpt-4".to_string());

        let google_api_key = std::env::var("GOOGLE_API_KEY");
        let google_search_engine_id = std::env::var("GOOGLE_SEARCH_ENGINE_ID");

        if google_api_key.is_err() || google_api_key == Ok("changeme".to_string()) {
            panic!("GOOGLE_API_KEY env var not configured. Check the golem.yaml for instructions");
        }

        if google_search_engine_id.is_err() || google_search_engine_id == Ok("changeme".to_string())
        {
            panic!("GOOGLE_SEARCH_ENGINE_ID env var not configured. Check the golem.yaml for instructions");
        }

        Self { model }
    }

    fn research(&self, topic: String) -> String {
        let search_results = search_web_for_topic(&topic);

        let search_results_json =
            serde_json::to_string(&search_results).unwrap_or_else(|_| "[]".to_string());

        let prompt = format!(
            "I'm writing a report on the topic \"{}\",\n\
             Your job is to be a research-assistant and provide me an initial overview on the topic so I can dive into it in more detail.\n\
             At the bottom are top search results from a search engine in json format. Use your own knowledge and the snippets from the search results to create the overview.\n\
             Also include the best links to look into to learn more about the topic. Prioritize objective and reliable sources.\n\
             \n\
             Search results: {}",
            topic, search_results_json
        );

        let config = Config {
            model: self.model.clone(),
            temperature: None,
            max_tokens: None,
            stop_sequences: None,
            tools: None,
            tool_choice: None,
            provider_options: None,
        };

        let events = vec![llm::Event::Message(Message {
            role: Role::Assistant,
            name: Some("research-agent".to_string()),
            content: vec![ContentPart::Text(prompt)],
        })];

        let response = llm::send(&events, &config).expect("Failed to send message to LLM");

        let text_result = response
            .content
            .iter()
            .filter_map(|content_part| match content_part {
                ContentPart::Text(txt) => Some(txt.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        format!("Finished research for topic {}:\n{}", topic, text_result)
    }
}

fn search_web_for_topic(topic: &str) -> Vec<SearchResult> {
    let pages_to_retrieve = 3;

    let session = web_search::start_search(&web_search::SearchParams {
        query: topic.to_string(),
        language: Some("lang_en".to_string()),
        safe_search: Some(types::SafeSearchLevel::Off),
        max_results: Some(10),
        time_range: None,
        include_domains: None,
        exclude_domains: None,
        include_images: None,
        include_html: None,
        advanced_answer: Some(true),
        region: None,
    })
        .expect("Failed to start search");

    let mut content: Vec<SearchResult> = Vec::new();

    for _ in 0..pages_to_retrieve {
        let page = session.next_page().expect("Failed to retrieve next page");

        for item in page {
            content.push(SearchResult {
                url: item.url.clone(),
                title: item.title.clone(),
                snippet: item.snippet.clone(),
            });
        }
    }

    content
}
