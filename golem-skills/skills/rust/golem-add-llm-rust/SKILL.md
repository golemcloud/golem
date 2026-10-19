---
name: golem-add-llm-rust
description: "Adds provider-backed AI capabilities to a Rust Golem agent. Use for LLMs, embeddings, search, vector or graph databases, speech, video, or code execution."
---

# Add AI capabilities to a Rust agent

The `golemcloud/golem-ai` project provides provider-neutral core crates and provider crates. Golem
agents now target `wasm32-wasip2` and WASI HTTP 0.3, so dependency selection matters.

## Compatibility status

Choose a `golem-ai` release that targets this repository's Rust SDK, `wasm32-wasip2`, and WASI
HTTP 0.3. The `0.5.1` and `0.5.2` releases predate that transport migration. Upstream has since
updated its transport and fallible Golem secret integration, but source support alone does not
establish that a particular release works with the SDK in the application.

Check the selected release's SDK dependency and build it in a current scaffold before documenting
or shipping it. Keep every selected core and provider crate on the same verified release or
revision. Do not disable durability features or copy an old provider example merely to make the
dependency resolve.

## Available crate families

Choose one core crate and the provider crate needed by the application:

| Capability | Core crate | Providers |
|---|---|---|
| LLM chat | `golem-ai-llm` | Anthropic, Bedrock, Grok, Ollama, OpenAI, OpenRouter, OrcaRouter (via OpenAI) |
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

## OrcaRouter through the OpenAI provider

Use `golem-ai-llm` with `golem-ai-llm-openai`; there is no separate OrcaRouter provider crate.
Set the provider's `base_url` to `Some("https://api.orcarouter.ai/v1".to_string())` and use an
OrcaRouter API key. The OpenAI provider appends `/responses`, so the base URL must include `/v1`
and must not end with a slash or include `/responses`.

With the explicit configuration API, obtain `OpenAiHostConfig` from agent configuration and
convert it to `OpenAiConfig`. Keep the API key as a Golem secret handle so the provider resolves
it for each request. Pass the provider configuration to `DurableOpenAI::send` or
`DurableOpenAI::stream`; setting environment variables alone does not configure these calls.

For applications that use `OpenAiConfig::from_env()`, the equivalent settings are
`OPENAI_API_KEY` and `OPENAI_BASE_URL=https://api.orcarouter.ai/v1`. The generated manifest
includes these as commented hints. The model is a separate setting: set `Config.model` to a model
ID from OrcaRouter's catalog. A base URL override does not translate model IDs.

Verify the operations the application needs against OrcaRouter, including Responses API streaming
and tool calls if used. OpenAI-compatible endpoints do not necessarily support every provider
feature.

## Safe alternative

If no compatible release has been verified, call a provider's HTTPS API with the supported outgoing HTTP
client described by `golem-make-http-request-rust`. Store credentials with Golem secrets; load
`golem-add-secret-rust` for provisioning. External calls made through supported host APIs remain
durable, while a generic third-party HTTP client may not integrate correctly with replay.

Verify the provider configuration, async call signature, and secret integration from the selected
release rather than relying on the examples from `0.5.2`.
