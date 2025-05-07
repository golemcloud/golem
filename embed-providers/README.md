# Golem Embedding Providers

This project implements WASM components for four embedding providers (OpenAI, Cohere, Hugging Face, and Voyage AI) following the `golem:embed@1.0.0` WIT interface. Each implementation includes custom durability semantics using the Golem durability API.

## Project Structure

The project is organized as a Rust workspace with the following structure:

```
embed-providers/
├── Cargo.toml (workspace)
├── wit/
│   └── embed.wit (WIT interface definition)
├── crates/
│   ├── embed-common/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs (common traits and utilities)
│   │       ├── durability.rs (durability operations)
│   │       └── wit.rs (WIT interface implementation helpers)
│   ├── embed-openai/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs (OpenAI client implementation)
│   │       └── wit.rs (WIT interface binding)
│   ├── embed-cohere/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs (Cohere client implementation)
│   │       └── wit.rs (WIT interface binding)
│   ├── embed-huggingface/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs (Hugging Face client implementation)
│   │       └── wit.rs (WIT interface binding)
│   └── embed-voyageai/
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs (Voyage AI client implementation)
│           └── wit.rs (WIT interface binding)
└── build.sh (build script for all components)
```

## Building the Components

To build all embedding providers as WASM components, run the build script:

```bash
# Make the build script executable
chmod +x build.sh

# Run the build script
./build.sh
```

This will:
1. Generate WIT bindings for the `golem:embed@1.0.0` interface
2. Compile each provider to a WASM module
3. Convert each module to a WASM component using `wasm-tools`
4. Output the components to the `target/wasm/` directory with the following names:
   - `golem_embed_openai.wasm`
   - `golem_embed_cohere.wasm`
   - `golem_embed_huggingface.wasm`
   - `golem_embed_voyageai.wasm`

## Configuration

Each provider requires an API key, which should be set as an environment variable:

- OpenAI: `OPENAI_API_KEY`
- Cohere: `COHERE_API_KEY`
- Hugging Face: `HUGGINGFACE_API_KEY`
- Voyage AI: `VOYAGEAI_API_KEY`

Additional configuration options can be passed through the WIT interface's `config` parameter.

## Features

### Embedding Generation

All providers implement the `generate-embeddings` function, which converts text inputs into vector embeddings.

### Reranking

All providers implement the `rerank` function, which reorders a list of documents based on their relevance to a query.

### Durability

Each provider implements custom durability semantics using the Golem durability API, allowing for operation replay in case of failures.

## Testing

Each provider includes comprehensive tests to verify functionality:

```bash
# Run tests for all providers
cargo test --workspace

# Run tests for a specific provider
cargo test -p embed-openai
```

## Compatibility

These components are designed to be compatible with Golem 1.2.x and use WASI 0.2.

## License

This project is licensed under the terms of the MIT license.