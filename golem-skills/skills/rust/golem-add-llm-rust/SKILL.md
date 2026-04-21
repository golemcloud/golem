---
name: golem-add-llm-rust
description: "Adding LLM and AI capabilities to a Rust Golem agent. Use when the user wants to add LLM chat, embeddings, web search, vector DB, graph DB, document search, video generation, speech-to-text, text-to-speech, or any AI provider integration."
---

# Adding LLM and AI Capabilities (Rust)

## Overview

Golem provides the **golem-ai** library collection — a set of Rust crates from [golemcloud/golem-ai](https://github.com/golemcloud/golem-ai) that provide unified, provider-agnostic APIs for AI capabilities. Each domain has a **core crate** (shared types and traits) plus **provider crates** (concrete backends). You add them as regular Cargo dependencies and call them directly from your agent code.

> **These crates are not on crates.io yet.** Use git dependencies pointing to the `dev` branch.

## Available Libraries

### LLM (Chat Completions)

Core crate: `golem-ai-llm` — unified chat completion API with blocking and streaming responses, multi-turn conversation, tool calling, and multimodal image inputs.

Provider crates (pick one):

| Provider | Crate | Required Env Vars |
|----------|-------|-------------------|
| OpenAI | `golem-ai-llm-openai` | `OPENAI_API_KEY` |
| Anthropic | `golem-ai-llm-anthropic` | `ANTHROPIC_API_KEY` |
| Amazon Bedrock | `golem-ai-llm-bedrock` | `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION` |
| xAI / Grok | `golem-ai-llm-grok` | `XAI_API_KEY` |
| Ollama | `golem-ai-llm-ollama` | `GOLEM_OLLAMA_BASE_URL` (optional, defaults to `http://localhost:11434`) |
| OpenRouter | `golem-ai-llm-openrouter` | `OPENROUTER_API_KEY` |

### Embeddings & Reranking

Core crate: `golem-ai-embed` — generate vector embeddings from text/images and rerank documents by relevance.

Provider crates:

| Provider | Crate | Required Env Vars |
|----------|-------|-------------------|
| OpenAI | `golem-ai-embed-openai` | `OPENAI_API_KEY` |
| Cohere | `golem-ai-embed-cohere` | `COHERE_API_KEY` |
| Hugging Face | `golem-ai-embed-hugging-face` | `HUGGING_FACE_API_KEY` |
| VoyageAI | `golem-ai-embed-voyageai` | `VOYAGEAI_API_KEY` |

### Web Search

Core crate: `golem-ai-web-search` — unified web search with one-shot and paginated session modes.

Provider crates:

| Provider | Crate | Required Env Vars |
|----------|-------|-------------------|
| Brave | `golem-ai-web-search-brave` | `BRAVE_API_KEY` |
| Google | `golem-ai-web-search-google` | `GOOGLE_API_KEY`, `GOOGLE_SEARCH_ENGINE_ID` |
| Serper | `golem-ai-web-search-serper` | `SERPER_API_KEY` |
| Tavily | `golem-ai-web-search-tavily` | `TAVILY_API_KEY` |

### Document Search

Core crate: `golem-ai-search` — full-text/document search with index management, document CRUD, faceted search.

Provider crates:

| Provider | Crate | Required Env Vars |
|----------|-------|-------------------|
| Algolia | `golem-ai-search-algolia` | `ALGOLIA_APPLICATION_ID`, `ALGOLIA_API_KEY` |
| Elasticsearch | `golem-ai-search-elasticsearch` | `ELASTICSEARCH_URL`, credentials |
| Meilisearch | `golem-ai-search-meilisearch` | `MEILISEARCH_BASE_URL`, `MEILISEARCH_API_KEY` |
| OpenSearch | `golem-ai-search-opensearch` | `OPENSEARCH_BASE_URL`, credentials |
| Typesense | `golem-ai-search-typesense` | `TYPESENSE_BASE_URL`, `TYPESENSE_API_KEY` |

### Graph Databases

Core crate: `golem-ai-graph` — vertex/edge CRUD, traversal, path-finding, transactions, schema management.

Provider crates:

| Provider | Crate |
|----------|-------|
| ArangoDB | `golem-ai-graph-arangodb` |
| JanusGraph | `golem-ai-graph-janusgraph` |
| Neo4j | `golem-ai-graph-neo4j` |

### Vector Databases

Core crate: `golem-ai-vector` — collection management, vector upsert/search, ANN queries, namespaces.

Provider crates:

| Provider | Crate |
|----------|-------|
| Qdrant | `golem-ai-vector-qdrant` |
| Milvus | `golem-ai-vector-milvus` |
| PgVector | `golem-ai-vector-pgvector` |
| Pinecone | `golem-ai-vector-pinecone` |

### Video Generation

Core crate: `golem-ai-video` — text-to-video, image-to-video, async job polling.

Provider crates:

| Provider | Crate |
|----------|-------|
| Google Veo | `golem-ai-video-veo` |
| Stability AI | `golem-ai-video-stability` |
| Kling | `golem-ai-video-kling` |
| Runway ML | `golem-ai-video-runway` |

### Speech-to-Text

Core crate: `golem-ai-stt` — audio transcription with speaker diarization, word-level timing.

Provider crates:

| Provider | Crate |
|----------|-------|
| OpenAI Whisper | `golem-ai-stt-whisper` |
| Deepgram | `golem-ai-stt-deepgram` |
| AWS Transcribe | `golem-ai-stt-aws` |
| Azure Speech | `golem-ai-stt-azure` |
| Google STT | `golem-ai-stt-google` |

### Text-to-Speech

Core crate: `golem-ai-tts` — voice discovery, batch/streaming synthesis, SSML support.

Provider crates:

| Provider | Crate |
|----------|-------|
| AWS Polly | `golem-ai-tts-aws` |
| Deepgram | `golem-ai-tts-deepgram` |
| ElevenLabs | `golem-ai-tts-elevenlabs` |
| Google Cloud TTS | `golem-ai-tts-google` |

## Adding Dependencies

Add the core crate plus your chosen provider to the component's `Cargo.toml`:

```toml
[dependencies]
# LLM — core + provider
golem-ai-llm = { git = "https://github.com/golemcloud/golem-ai", branch = "dev" }
golem-ai-llm-openai = { git = "https://github.com/golemcloud/golem-ai", branch = "dev" }
```

Store the required API key as a **secret** using Golem's typed config system. Load the `golem-add-secret-rust` skill for full details. In brief:

```rust
use golem_rust::ConfigSchema;
use golem_rust::agentic::{Config, Secret};

#[derive(ConfigSchema)]
pub struct MyAgentConfig {
    #[config_schema(secret)]
    pub api_key: Secret<String>,
}
```

Then manage the secret via the CLI:

```shell
golem agent-secret create api_key --secret-type String --secret-value "sk-..."
```

## Usage: LLM Chat Completion

```rust
use golem_ai_llm::model::*;
use golem_ai_llm::LlmProvider;

// Pick a provider — type alias makes it easy to swap later
type Provider = golem_ai_llm_openai::DurableOpenAI;

let config = Config {
    model: "gpt-4o".to_string(),
    temperature: None,
    max_tokens: None,
    stop_sequences: None,
    tools: None,
    tool_choice: None,
    provider_options: None,
};

let events = vec![Event::Message(Message {
    role: Role::User,
    name: None,
    content: vec![ContentPart::Text("Hello!".to_string())],
})];

// Blocking request
let response = Provider::send(events, config).expect("LLM call failed");

// Extract text from response
let text: String = response
    .content
    .iter()
    .filter_map(|part| match part {
        ContentPart::Text(txt) => Some(txt.clone()),
        _ => None,
    })
    .collect::<Vec<_>>()
    .join("\n");
```

## Usage: Multi-turn Conversation (Session)

```rust
use golem_ai_llm::model::*;
use golem_ai_llm::LlmProvider;

type Provider = golem_ai_llm_openai::DurableOpenAI;

// Keep events as agent state for multi-turn conversation
let mut events: Vec<Event> = vec![];

// Add system message
events.push(Event::Message(Message {
    role: Role::System,
    name: None,
    content: vec![ContentPart::Text("You are a helpful assistant.".to_string())],
}));

// Add user message
events.push(Event::Message(Message {
    role: Role::User,
    name: None,
    content: vec![ContentPart::Text("What is Golem?".to_string())],
}));

let config = Config {
    model: "gpt-4o".to_string(),
    temperature: None,
    max_tokens: None,
    stop_sequences: None,
    tools: None,
    tool_choice: None,
    provider_options: None,
};

// Send and record the response for next turn
let response = Provider::send(events.clone(), config.clone()).expect("LLM call failed");
events.push(Event::Response(response));
```

## Usage: Web Search

```rust
use golem_ai_web_search::model::types;
use golem_ai_web_search::model::web_search;

type SearchProvider = golem_ai_web_search_google::DurableGoogleCustomSearch;

let session = SearchProvider::start_search(&web_search::SearchParams {
    query: "Golem distributed computing".to_string(),
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
}).expect("Failed to start search");

let results = session.next_page().expect("Failed to get results");
for result in results {
    println!("{}: {}", result.title, result.url);
}
```

## Switching Providers

To switch providers, change the type alias and dependency. The core API stays the same:

```rust
// Switch from OpenAI to Anthropic:
// 1. In Cargo.toml: replace golem-ai-llm-openai with golem-ai-llm-anthropic
// 2. In code:
type Provider = golem_ai_llm_anthropic::DurableAnthropic;
// All other code stays the same
```

## Complete Agent Example

```rust
use golem_ai_llm::model::*;
use golem_ai_llm::LlmProvider;
use golem_rust::{agent_definition, agent_implementation, endpoint};

type Provider = golem_ai_llm_openai::DurableOpenAI;

#[agent_definition(mount = "/chats/{chat_name}")]
pub trait ChatAgent {
    fn new(chat_name: String) -> Self;

    #[endpoint(post = "/ask")]
    async fn ask(&mut self, question: String) -> String;
}

struct ChatAgentImpl {
    chat_name: String,
    events: Vec<Event>,
    config: Config,
}

#[agent_implementation]
impl ChatAgent for ChatAgentImpl {
    fn new(chat_name: String) -> Self {
        let config = Config {
            model: std::env::var("LLM_MODEL").unwrap_or_else(|_| "gpt-4o".to_string()),
            temperature: None,
            max_tokens: None,
            stop_sequences: None,
            tools: None,
            tool_choice: None,
            provider_options: None,
        };
        let events = vec![Event::Message(Message {
            role: Role::System,
            name: None,
            content: vec![ContentPart::Text(format!(
                "You are a helpful assistant for chat '{}'",
                chat_name
            ))],
        })];
        Self { chat_name, events, config }
    }

    async fn ask(&mut self, question: String) -> String {
        self.events.push(Event::Message(Message {
            role: Role::User,
            name: None,
            content: vec![ContentPart::Text(question)],
        }));

        let response = Provider::send(self.events.clone(), self.config.clone())
            .expect("LLM call failed");
        self.events.push(Event::Response(response.clone()));

        response
            .content
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text(txt) => Some(txt.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
```

## Key Constraints

- All golem-ai crates must be added as git dependencies from `https://github.com/golemcloud/golem-ai` with `branch = "dev"` — they are not on crates.io yet
- Always add both the core crate and a provider crate (e.g., `golem-ai-llm` + `golem-ai-llm-openai`)
- Provider API keys should be stored as secrets using Golem's typed config system (load the `golem-add-secret-rust` skill)
- The `Durable*` provider types (e.g., `DurableOpenAI`) automatically integrate with Golem's durable execution — responses are recorded in the oplog and replayed on recovery
- To switch providers, change the type alias and Cargo dependency — the rest of the code stays the same
- These crates target `wasm32-wasip1` and work correctly in Golem's WebAssembly environment
