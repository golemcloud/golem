---
name: golem-add-llm-rust
description: "Adds provider-backed AI capabilities to a Rust Golem agent. Use for LLMs, embeddings, search, vector or graph databases, speech, video, or code execution."
---

# Add AI capabilities to a Rust agent

The `golemcloud/golem-ai` project provides provider-neutral core crates and provider crates. Golem
agents now target `wasm32-wasip2` and WASI HTTP 0.3, so dependency selection matters.

## Compatibility status

There is currently no verified `golem-ai` release or revision compatible with this repository's
Rust SDK. The latest crates.io release, `0.5.2`, predates the WASIp2/WASI P3 migration. The current
upstream source targets `wasm32-wasip2`, but still expects the older infallible Golem secret API and
does not compile against this SDK with its default Golem integration enabled.

Do not add `0.5.2`, pin the upstream migration commit, disable durability features, or copy an old
provider example merely to make the dependency resolve. Recheck upstream releases and build the
chosen version in a current scaffold before documenting or shipping it. Keep every selected core
and provider crate on the same verified release or revision.

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

## Safe alternative

Until a compatible release exists, call a provider's HTTPS API with the supported outgoing HTTP
client described by `golem-make-http-request-rust`. Store credentials with Golem secrets; load
`golem-add-secret-rust` for provisioning. External calls made through supported host APIs remain
durable, while a generic third-party HTTP client may not integrate correctly with replay.

When upstream compatibility is restored, verify the provider configuration, async call signature,
and secret integration from that exact release rather than relying on the examples from `0.5.2`.
