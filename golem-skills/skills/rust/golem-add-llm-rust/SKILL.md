---
name: golem-add-llm-rust
description: "Adds provider-backed AI capabilities to a Rust Golem agent. Use for LLMs, embeddings, search, vector or graph databases, speech, video, or code execution."
---

# Add AI capabilities to a Rust agent

The `golemcloud/golem-ai` project provides provider-neutral core crates and provider crates. Golem
agents now target `wasm32-wasip2` and WASI HTTP 0.3, so dependency selection matters.

## Select matching dependencies

Create a Rust application with `golem new --yes --template rust ai-agent`. Add the core and
provider dependencies to the component's `Cargo.toml`. This example uses the
[v0.5.4 source](https://github.com/golemcloud/golem-ai/tree/9243c29395a4cd85f81f9c7c635d8b0e3fe49216)
with its matching SDK:

```toml
[dependencies]
golem-rust = { version = "=3.0.0-rc5", features = ["export_golem_agentic"] }
golem-ai-llm = { git = "https://github.com/golemcloud/golem-ai", rev = "9243c29395a4cd85f81f9c7c635d8b0e3fe49216" }
golem-ai-llm-openai = { git = "https://github.com/golemcloud/golem-ai", rev = "9243c29395a4cd85f81f9c7c635d8b0e3fe49216" }
```

Keep the default `golem` features enabled. The application and provider must use the same SDK;
check `cargo tree -d` for duplicate SDK versions. For a local SDK build, use the source-checkout
instructions below instead. Build from the application directory with `golem build --yes` and
keep the resulting lockfile.

## Configure and call OpenAI

The provider exports `OpenAiHostConfig` (a `ConfigSchema`) and `OpenAiConfig` (the runtime config).
Inject `Config<OpenAiHostConfig>` through the constructor. Converting the loaded host config keeps
the secret handle; the provider reveals it immediately before each request. Do not cache plaintext.

```rust
use golem_rust::{agent_definition, agent_implementation};
use golem_rust::agentic::Config as AgentConfig;
use golem_ai_llm::{LlmProvider, model::{Config, ContentPart, Event, Message, Role}};
use golem_ai_llm_openai::{OpenAI, OpenAiConfig, OpenAiHostConfig};

#[agent_definition]
pub trait ChatAgent {
    fn new(name: String, #[agent_config] config: AgentConfig<OpenAiHostConfig>) -> Self;
    async fn chat(&self, prompt: String, model: String) -> Result<String, String>;
}

struct ChatAgentImpl {
    config: AgentConfig<OpenAiHostConfig>,
}

#[agent_implementation]
impl ChatAgent for ChatAgentImpl {
    fn new(_name: String, #[agent_config] config: AgentConfig<OpenAiHostConfig>) -> Self {
        Self { config }
    }

    async fn chat(&self, prompt: String, model: String) -> Result<String, String> {
        let host = self.config.get().map_err(|_| "Provider configuration unavailable".to_string())?;
        let response = OpenAI::send(
            OpenAiConfig::from(host),
            vec![Event::Message(Message {
                role: Role::User,
                name: None,
                content: vec![ContentPart::Text(prompt)],
            })],
            Config {
                model,
                temperature: None,
                max_tokens: Some(256),
                stop_sequences: None,
                tools: None,
                tool_choice: None,
                provider_options: None,
            },
        ).await.map_err(|_| "Provider request failed".to_string())?;
        Ok(response.content.into_iter().filter_map(|part| match part {
            ContentPart::Text(text) => Some(text),
            _ => None,
        }).collect::<Vec<_>>().join(""))
    }
}
```

Provision the `apiKey` secret in the deployment environment, using the hidden prompt rather than
putting a credential in the manifest or shell history:

```shell
golem secret create apiKey --type String
```

Set the optional provider URL config explicitly in `golem.yaml`:

```yaml
agents:
  ChatAgent:
    config:
      base_url: "https://openai-proxy.example.com/v1"
```

Regular config uses the Rust field name `base_url`; the secret path is `apiKey`.
Replace the example proxy URL with a reachable OpenAI-compatible endpoint.

Build, deploy, and call `chat` with a model available to the provider account. Check the returned
domain error separately from invocation failure. For streaming, use `LlmProvider::stream` and the
upstream chat-stream interface. Before production use, verify provider requests, secret rotation,
and recovery.

Authoritative API references at the pinned revision:
[provider config](https://github.com/golemcloud/golem-ai/blob/9243c29395a4cd85f81f9c7c635d8b0e3fe49216/llm/openai/src/config.rs),
[provider implementation](https://github.com/golemcloud/golem-ai/blob/9243c29395a4cd85f81f9c7c635d8b0e3fe49216/llm/openai/src/lib.rs),
[request model](https://github.com/golemcloud/golem-ai/blob/9243c29395a4cd85f81f9c7c635d8b0e3fe49216/llm/llm/src/model.rs).

## Available crate families

Choose one core crate and the provider crate needed by the application:

| Capability | Core crate | Providers |
|---|---|---|
| LLM chat | `golem-ai-llm` | Anthropic, Bedrock, Grok, Ollama, OpenAI, OpenRouter |
| Embeddings/reranking | `golem-ai-embed` | Cohere, Hugging Face, OpenAI, VoyageAI |
| Web search | `golem-ai-web-search` | Brave, Google, Serper, Tavily |
| Document search | `golem-ai-search` | Algolia, Elasticsearch, Meilisearch, OpenSearch, Typesense |
| Graph database | `golem-ai-graph` | ArangoDB, JanusGraph, Neo4j |
| Vector database | `golem-ai-vector` | Milvus, pgvector, Pinecone, Qdrant |
| Video | `golem-ai-video` | Kling, Runway, Stability, Veo |
| Speech-to-text | `golem-ai-stt` | AWS, Azure, Deepgram, Google, Whisper |
| Text-to-speech | `golem-ai-tts` | AWS, Deepgram, ElevenLabs, Google |
| Code execution | `golem-ai-exec` | JavaScript and Python execution |

The repository also contains `golem-ai-http`, the shared WASI HTTP transport. Provider crate names
follow `golem-ai-<capability>-<provider>`.

## Direct HTTP alternative

For a provider without a suitable adapter, call its HTTPS API with `golem_rust::http` as described
by `golem-make-http-request-rust`. Store credentials with Golem secrets; load
`golem-add-secret-rust` for provisioning.

## Use a local SDK checkout

When targeting Golem's development SDK, clone `golemcloud/golem-ai` at the revision above. In
the provider workspace's `[workspace.dependencies]`, replace `golem-rust` with
`{ path = "/absolute/path/to/golem/sdks/rust/golem-rust" }`. Point the application at the same
SDK and provider checkout:

```toml
[dependencies]
golem-rust = { path = "/absolute/path/to/golem/sdks/rust/golem-rust", features = ["export_golem_agentic"] }
golem-ai-llm = { path = "/absolute/path/to/golem-ai/llm/llm" }
golem-ai-llm-openai = { path = "/absolute/path/to/golem-ai/llm/openai" }
```

Change the provider workspace dependency directly: a crates.io patch to the development SDK's
`0.0.0` version does not satisfy the provider's `=3.0.0-rc5` requirement.
